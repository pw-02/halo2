//! This module provides common utilities, traits and structures for group,
//! field and polynomial arithmetic.

use super::multicore;
pub use ff::Field;
use group::{
    ff::{BatchInvert, PrimeField},
    Curve, Group, GroupOpsOwned, ScalarMulOwned,
};
pub use halo2curves::{CurveAffine, CurveExt};
// #[cfg(any(feature = "cuda", feature = "opencl"))]
// use ec_gpu_gen::fft::FftKernel;
// #[cfg(any(feature = "cuda", feature = "opencl"))]
// use crate::gpu;
// use ec_gpu_gen::fft_cpu;
// use ec_gpu_gen::threadpool::Worker;

#[cfg(feature = "gpu")]
use {
    ec_gpu_gen,
    ec_gpu_gen::rust_gpu_tools::{program_closures, Device, Program, Vendor, CUDA_CORES},
    ec_gpu_gen::fft::FftKernel,
    halo2curves::bn256::Bn256,
    ec_gpu_gen::threadpool::Worker,
    ec_gpu_gen::multiexp::MultiexpKernel,
    std::sync::Arc,
};



#[cfg(feature = "icicle_gpu")]
use super::icicle;
#[cfg(feature = "icicle_gpu")]
use rustacuda::prelude::DeviceBuffer;
use csv::Writer;
use std::path::Path;
use serde::Serialize;
use std::time::Instant;
use std::error::Error;

#[derive(Serialize, Debug)]
struct FFTLoggingInfo {     
    size: u32,
    logn: u32,
    fft_duration: f64,
    device: String,
}

impl FFTLoggingInfo {
    // Constructor for FFTLoggingInfo
    fn new(size: u32, logn: u32, fft_duration: f64, device: &str) -> Self {
        FFTLoggingInfo {
            size,
            logn,
            fft_duration,
            device: device.to_string(),
        }
    }
}

#[derive(Serialize, Debug)]
struct MSMLoggingInfo {     
    num_coeffs: u32,
    msm_duration: f64,
    device: String,

}

fn log_fft_stats(stat_collector:FFTLoggingInfo)-> Result<(), Box<dyn Error>>
{  
    let filename = "halo2_ffts.csv";
    let file_exists = Path::new(filename).exists();
    // Open the file in append mode, create it if it does not exist
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(filename)?;

    // Create a CSV writer
    let mut wtr = Writer::from_writer(file);


    if !file_exists {
        wtr.write_record(&["size", "log_n", "device", "duration(s)"])?;
    }
    // Write the record with proper type conversion
    wtr.write_record(&[
        stat_collector.size.to_string(),
        stat_collector.logn.to_string(),
        stat_collector.device,
        stat_collector.fft_duration.to_string(),
    ])?;
    wtr.flush()?;
    Ok(())
 
}

fn log_msm_stats(stat_collector:MSMLoggingInfo)-> Result<(), Box<dyn Error>>
{   
    let filename = "halo2_msms.csv";
    let file_exists = Path::new(filename).exists();
    // Open or create the file
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(filename)?;
    // Create a CSV writer
      
    let mut wtr = csv::Writer::from_writer(file);

      // Write header if the file does not already exist
      if !file_exists {
          wtr.write_record(&["num_coeffs", "device", "duration(s)"])?;
      }
    
    // Write the logging information
    wtr.write_record(&[
        &stat_collector.num_coeffs.to_string(),
        &stat_collector.device.to_string(),
        &stat_collector.msm_duration.to_string(),


    ])?;
    // Ensure all data is written to the file
    wtr.flush()?;
    Ok(())
}



/// This represents an element of a group with basic operations that can be
/// performed. This allows an FFT implementation (for example) to operate
/// generically over either a field or elliptic curve group.
pub trait FftGroup<Scalar: Field>:
    Copy + Send + Sync + 'static + GroupOpsOwned + ScalarMulOwned<Scalar>
{
}

impl<T, Scalar> FftGroup<Scalar> for T
where
    Scalar: Field,
    T: Copy + Send + Sync + 'static + GroupOpsOwned + ScalarMulOwned<Scalar>,
{
}

fn multiexp_serial<C: CurveAffine>(coeffs: &[C::Scalar], bases: &[C], acc: &mut C::Curve) {
    let coeffs: Vec<_> = coeffs.iter().map(|a| a.to_repr()).collect();

    let c = if bases.len() < 4 {
        1
    } else if bases.len() < 32 {
        3
    } else {
        (f64::from(bases.len() as u32)).ln().ceil() as usize
    };

    fn get_at<F: PrimeField>(segment: usize, c: usize, bytes: &F::Repr) -> usize {
        let skip_bits = segment * c;
        let skip_bytes = skip_bits / 8;

        if skip_bytes >= (F::NUM_BITS as usize + 7) / 8 {
            return 0;
        }

        let mut v = [0; 8];
        for (v, o) in v.iter_mut().zip(bytes.as_ref()[skip_bytes..].iter()) {
            *v = *o;
        }

        let mut tmp = u64::from_le_bytes(v);
        tmp >>= skip_bits - (skip_bytes * 8);
        tmp %= 1 << c;

        tmp as usize
    }

    let segments = (C::Scalar::NUM_BITS as usize / c) + 1;

    for current_segment in (0..segments).rev() {
        for _ in 0..c {
            *acc = acc.double();
        }

        #[derive(Clone, Copy)]
        enum Bucket<C: CurveAffine> {
            None,
            Affine(C),
            Projective(C::Curve),
        }

        impl<C: CurveAffine> Bucket<C> {
            fn add_assign(&mut self, other: &C) {
                *self = match *self {
                    Bucket::None => Bucket::Affine(*other),
                    Bucket::Affine(a) => Bucket::Projective(a + *other),
                    Bucket::Projective(mut a) => {
                        a += *other;
                        Bucket::Projective(a)
                    }
                }
            }

            fn add(self, mut other: C::Curve) -> C::Curve {
                match self {
                    Bucket::None => other,
                    Bucket::Affine(a) => {
                        other += a;
                        other
                    }
                    Bucket::Projective(a) => other + &a,
                }
            }
        }

        let mut buckets: Vec<Bucket<C>> = vec![Bucket::None; (1 << c) - 1];

        for (coeff, base) in coeffs.iter().zip(bases.iter()) {
            let coeff = get_at::<C::Scalar>(current_segment, c, coeff);
            if coeff != 0 {
                buckets[coeff - 1].add_assign(base);
            }
        }

        // Summation by parts
        // e.g. 3a + 2b + 1c = a +
        //                    (a) + b +
        //                    ((a) + b) + c
        let mut running_sum = C::Curve::identity();
        for exp in buckets.into_iter().rev() {
            running_sum = exp.add(running_sum);
            *acc += &running_sum;
        }
    }
}

/// Performs a small multi-exponentiation operation.
/// Uses the double-and-add algorithm with doublings shared across points.
pub fn small_multiexp<C: CurveAffine>(coeffs: &[C::Scalar], bases: &[C]) -> C::Curve {
    let coeffs: Vec<_> = coeffs.iter().map(|a| a.to_repr()).collect();
    let mut acc = C::Curve::identity();

    // for byte idx
    for byte_idx in (0..((C::Scalar::NUM_BITS as usize + 7) / 8)).rev() {
        // for bit idx
        for bit_idx in (0..8).rev() {
            acc = acc.double();
            // for each coeff
            for coeff_idx in 0..coeffs.len() {
                let byte = coeffs[coeff_idx].as_ref()[byte_idx];
                if ((byte >> bit_idx) & 1) != 0 {
                    acc += bases[coeff_idx];
                }
            }
        }
    }

    acc
}

// /// Performs a FFFT operation on GPU
// #[cfg(feature = "icicle_gpu")]
// pub fn best_fft_gpu<Scalar: Field, G: FftGroup<Scalar>>(
//     a: &mut [G],
//     omega: Scalar,
//     log_n: u32,
// ) {
//     icicle::ntt::
//     icicle::fft_on_device::<Scalar, G>(a, omega, log_n);
//     let d = 1 << log_n;
//     // Using default config
//     let cfg = ntt::NTTConfig::<Bn254ScalarField>::default();
// }

#[cfg(feature = "icicle_gpu")]
/// Performs a multi-exponentiation operation on GPU using Icicle library
pub fn best_multiexp_gpu<C: CurveAffine>(coeffs: &[C::Scalar], is_lagrange: bool) -> C::Curve {
    let scalars_ptr: DeviceBuffer<::icicle::curves::bn254::ScalarField_BN254> =
        icicle::copy_scalars_to_device::<C>(coeffs);

    return icicle::multiexp_on_device::<C>(scalars_ptr, is_lagrange);
}

/// Performs a multi-exponentiation operation.
///
/// This function will panic if coeffs and bases have a different length.
///
/// This will use multithreading if beneficial.
pub fn cpu_multiexp<C: CurveAffine>(coeffs: &[C::Scalar], bases: &[C]) -> C::Curve {
    assert_eq!(coeffs.len(), bases.len());

    let mut stat_collector = MSMLoggingInfo{
        num_coeffs: coeffs.len() as u32,
        msm_duration: 0.0,
        device: String::from("cpu"),
    };

    let num_threads = multicore::current_num_threads();
    let start_time = Instant::now();

    let result = if coeffs.len() > num_threads {
        let chunk = coeffs.len() / num_threads;
        let num_chunks = coeffs.chunks(chunk).len();
        let mut results = vec![C::Curve::identity(); num_chunks];
        multicore::scope(|scope| {
            let chunk = coeffs.len() / num_threads;

            for ((coeffs, bases), acc) in coeffs
                .chunks(chunk)
                .zip(bases.chunks(chunk))
                .zip(results.iter_mut())
            {
                scope.spawn(move |_| {
                    multiexp_serial(coeffs, bases, acc);
                });
            }
        });
        results.iter().fold(C::Curve::identity(), |a, b| a + b)
    } else {
        let mut acc = C::Curve::identity();
        multiexp_serial(coeffs, bases, &mut acc);
        acc
    };
    let total_msm_time = start_time.elapsed();
    stat_collector.msm_duration = total_msm_time.as_secs_f64();
    // Handle potential logging errors
    if let Err(e) = log_msm_stats(stat_collector) {
        eprintln!("Failed to log MSM stats: {}", e);
    }
    result

}

pub fn gpu_multiexp<C: CurveAffine>(coeffs: &[C::Scalar], bases: &[C]) -> Result<C::Curve, ec_gpu_gen::EcError>{

    assert_eq!(coeffs.len(), bases.len());

    let mut stat_collector = MSMLoggingInfo{
        num_coeffs: coeffs.len() as u32,
        msm_duration: 0.0,
        device: String::from("cpu"),
    };
    let start_time = Instant::now();
    let devices = Device::all();
    let mut kern = MultiexpKernel::<Bn256>::create(&devices).expect("Cannot initialize kernel!");

    let pool = Worker::new();
    let t: Arc<Vec<_>> = Arc::new(coeffs.iter().map(|a| a.to_repr()).collect());
    let g:Arc<Vec<_>> = Arc::new(bases.to_vec().clone());
    let g2 = (g.clone(), 0);
    let (bss, skip) =  (g2.0.clone(), g2.1);
    let result = kern.multiexp(&pool, bss, t, skip).map_err(Into::into);
    let total_msm_time = start_time.elapsed();
    stat_collector.msm_duration = total_msm_time.as_secs_f64();
    // Handle potential logging errors
    if let Err(e) = log_msm_stats(stat_collector) {
        eprintln!("Failed to log MSM stats: {}", e);
    }
    result
}


pub fn best_multiexp<C: CurveAffine>(coeffs: &[C::Scalar], bases: &[C]) -> C::Curve {
    #[cfg(feature = "gpu")]
    let result = gpu_multiexp(coeffs, bases).unwrap();

    #[cfg(not(any(feature = "gpu", feature = "opencl")))]
    let result = cpu_multiexp(coeffs, bases);

    result
}



/// Performs a radix-$2$ Fast-Fourier Transformation (FFT) on a vector of size
/// $n = 2^k$, when provided `log_n` = $k$ and an element of multiplicative
/// order $n$ called `omega` ($\omega$). The result is that the vector `a`, when
/// interpreted as the coefficients of a polynomial of degree $n - 1$, is
/// transformed into the evaluations of this polynomial at each of the $n$
/// distinct powers of $\omega$. This transformation is invertible by providing
/// $\omega^{-1}$ in place of $\omega$ and dividing each resulting field element
/// by $n$.
///
/// This will use multithreading if beneficial.
pub fn best_fft<Scalar: Field, G: FftGroup<Scalar>>(a: &mut [G], omega: Scalar, log_n: u32) {
    #[cfg(feature = "gpu")]
    gpu_fft(a, omega, log_n);

    #[cfg(not(any(feature = "gpu", feature = "opencl")))]
    cpu_fft(a, omega, log_n);
}

pub fn gpu_fft<Scalar: Field, G: FftGroup<Scalar>>(a: &mut [G], omega: Scalar, log_n: u32) {
    
    let mut stat_collector = FFTLoggingInfo::new(
        a.len() as u32,
        log_n,
        0.0, // placeholder for fft_duration
        "gpu"
    );
    let timer = Instant::now();
    let devices = Device::all();
    let mut kern = FftKernel::<Bn256>::create(&devices).expect("Cannot initialize kernel!");
    kern.radix_fft_many(&mut [a], &[omega], &[log_n]).expect("GPU FFT failed!");

    let total_fft_time = timer.elapsed();
    stat_collector.fft_duration = total_fft_time.as_secs_f64();
    let _ = log_fft_stats(stat_collector);
}

pub fn cpu_fft<Scalar: Field, G: FftGroup<Scalar>>(a: &mut [G], omega: Scalar, log_n: u32) {
    
    let mut stat_collector = FFTLoggingInfo::new(
        a.len() as u32,
        log_n,
        0.0, // placeholder for fft_duration
        "cpu"
    );

    let timer = Instant::now();

    
    fn bitreverse(mut n: usize, l: usize) -> usize {
        let mut r = 0;
        for _ in 0..l {
            r = (r << 1) | (n & 1);
            n >>= 1;
        }
        r
    }

    let threads = multicore::current_num_threads();
    let log_threads = log2_floor(threads);
    let n = a.len();
    assert_eq!(n, 1 << log_n);

    for k in 0..n {
        let rk = bitreverse(k, log_n as usize);
        if k < rk {
            a.swap(rk, k);
        }
    }

    // precompute twiddle factors
    let twiddles: Vec<_> = (0..(n / 2))
        .scan(Scalar::ONE, |w, _| {
            let tw = *w;
            *w *= &omega;
            Some(tw)
        })
        .collect();

    if log_n <= log_threads {
        let mut chunk = 2_usize;
        let mut twiddle_chunk = n / 2;
        for _ in 0..log_n {
            a.chunks_mut(chunk).for_each(|coeffs| {
                let (left, right) = coeffs.split_at_mut(chunk / 2);

                // case when twiddle factor is one
                let (a, left) = left.split_at_mut(1);
                let (b, right) = right.split_at_mut(1);
                let t = b[0];
                b[0] = a[0];
                a[0] += &t;
                b[0] -= &t;

                left.iter_mut()
                    .zip(right.iter_mut())
                    .enumerate()
                    .for_each(|(i, (a, b))| {
                        let mut t = *b;
                        t *= &twiddles[(i + 1) * twiddle_chunk];
                        *b = *a;
                        *a += &t;
                        *b -= &t;
                    });
            });
            chunk *= 2;
            twiddle_chunk /= 2;
        }
    } else {
        recursive_butterfly_arithmetic(a, n, 1, &twiddles)
    }

    let total_fft_time = timer.elapsed();
    stat_collector.fft_duration = total_fft_time.as_secs_f64();
    let _ = log_fft_stats(stat_collector);
}


// pub fn best_fft<Scalar: Field, G: FftGroup<Scalar>>(a: &mut [G], omega: Scalar, log_n: u32) {
    
//     let mut stat_collector = FFTLoggingInfo::new(
//         a.len() as u32,
//         log_n,
//         0.0, // placeholder for fft_duration
//         "cpu"
//     );

//     let timer = Instant::now();

    
//     fn bitreverse(mut n: usize, l: usize) -> usize {
//         let mut r = 0;
//         for _ in 0..l {
//             r = (r << 1) | (n & 1);
//             n >>= 1;
//         }
//         r
//     }

//     let threads = multicore::current_num_threads();
//     let log_threads = log2_floor(threads);
//     let n = a.len();
//     assert_eq!(n, 1 << log_n);

//     for k in 0..n {
//         let rk = bitreverse(k, log_n as usize);
//         if k < rk {
//             a.swap(rk, k);
//         }
//     }

//     // precompute twiddle factors
//     let twiddles: Vec<_> = (0..(n / 2))
//         .scan(Scalar::ONE, |w, _| {
//             let tw = *w;
//             *w *= &omega;
//             Some(tw)
//         })
//         .collect();

//     if log_n <= log_threads {
//         let mut chunk = 2_usize;
//         let mut twiddle_chunk = n / 2;
//         for _ in 0..log_n {
//             a.chunks_mut(chunk).for_each(|coeffs| {
//                 let (left, right) = coeffs.split_at_mut(chunk / 2);

//                 // case when twiddle factor is one
//                 let (a, left) = left.split_at_mut(1);
//                 let (b, right) = right.split_at_mut(1);
//                 let t = b[0];
//                 b[0] = a[0];
//                 a[0] += &t;
//                 b[0] -= &t;

//                 left.iter_mut()
//                     .zip(right.iter_mut())
//                     .enumerate()
//                     .for_each(|(i, (a, b))| {
//                         let mut t = *b;
//                         t *= &twiddles[(i + 1) * twiddle_chunk];
//                         *b = *a;
//                         *a += &t;
//                         *b -= &t;
//                     });
//             });
//             chunk *= 2;
//             twiddle_chunk /= 2;
//         }
//     } else {
//         recursive_butterfly_arithmetic(a, n, 1, &twiddles)
//     }

//     let total_fft_time = timer.elapsed();
//     stat_collector.fft_duration = total_fft_time.as_secs_f64();
//     let _ = log_fft_stats(stat_collector);
// }

/// This perform recursive butterfly arithmetic
pub fn recursive_butterfly_arithmetic<Scalar: Field, G: FftGroup<Scalar>>(
    a: &mut [G],
    n: usize,
    twiddle_chunk: usize,
    twiddles: &[Scalar],
) {
    if n == 2 {
        let t = a[1];
        a[1] = a[0];
        a[0] += &t;
        a[1] -= &t;
    } else {
        let (left, right) = a.split_at_mut(n / 2);
        multicore::join(
            || recursive_butterfly_arithmetic(left, n / 2, twiddle_chunk * 2, twiddles),
            || recursive_butterfly_arithmetic(right, n / 2, twiddle_chunk * 2, twiddles),
        );

        // case when twiddle factor is one
        let (a, left) = left.split_at_mut(1);
        let (b, right) = right.split_at_mut(1);
        let t = b[0];
        b[0] = a[0];
        a[0] += &t;
        b[0] -= &t;

        left.iter_mut()
            .zip(right.iter_mut())
            .enumerate()
            .for_each(|(i, (a, b))| {
                let mut t = *b;
                t *= &twiddles[(i + 1) * twiddle_chunk];
                *b = *a;
                *a += &t;
                *b -= &t;
            });
    }
}

/// Convert coefficient bases group elements to lagrange basis by inverse FFT.
pub fn g_to_lagrange<C: CurveAffine>(g_projective: Vec<C::Curve>, k: u32) -> Vec<C> {
    let n_inv = C::Scalar::TWO_INV.pow_vartime([k as u64, 0, 0, 0]);
    let mut omega_inv = C::Scalar::ROOT_OF_UNITY_INV;
    for _ in k..C::Scalar::S {
        omega_inv = omega_inv.square();
    }

    let mut g_lagrange_projective = g_projective;
    best_fft(&mut g_lagrange_projective, omega_inv, k);
    parallelize(&mut g_lagrange_projective, |g, _| {
        for g in g.iter_mut() {
            *g *= n_inv;
        }
    });

    let mut g_lagrange = vec![C::identity(); 1 << k];
    parallelize(&mut g_lagrange, |g_lagrange, starts| {
        C::Curve::batch_normalize(
            &g_lagrange_projective[starts..(starts + g_lagrange.len())],
            g_lagrange,
        );
    });

    g_lagrange
}

/// This evaluates a provided polynomial (in coefficient form) at `point`.
pub fn eval_polynomial<F: Field>(poly: &[F], point: F) -> F {
    fn evaluate<F: Field>(poly: &[F], point: F) -> F {
        poly.iter()
            .rev()
            .fold(F::ZERO, |acc, coeff| acc * point + coeff)
    }
    let n = poly.len();
    let num_threads = multicore::current_num_threads();
    if n * 2 < num_threads {
        evaluate(poly, point)
    } else {
        let chunk_size = (n + num_threads - 1) / num_threads;
        let mut parts = vec![F::ZERO; num_threads];
        multicore::scope(|scope| {
            for (chunk_idx, (out, poly)) in
                parts.chunks_mut(1).zip(poly.chunks(chunk_size)).enumerate()
            {
                scope.spawn(move |_| {
                    let start = chunk_idx * chunk_size;
                    out[0] = evaluate(poly, point) * point.pow_vartime([start as u64, 0, 0, 0]);
                });
            }
        });
        parts.iter().fold(F::ZERO, |acc, coeff| acc + coeff)
    }
}

/// This computes the inner product of two vectors `a` and `b`.
///
/// This function will panic if the two vectors are not the same size.
pub fn compute_inner_product<F: Field>(a: &[F], b: &[F]) -> F {
    // TODO: parallelize?
    assert_eq!(a.len(), b.len());

    let mut acc = F::ZERO;
    for (a, b) in a.iter().zip(b.iter()) {
        acc += (*a) * (*b);
    }

    acc
}

/// Divides polynomial `a` in `X` by `X - b` with
/// no remainder.
pub fn kate_division<'a, F: Field, I: IntoIterator<Item = &'a F>>(a: I, mut b: F) -> Vec<F>
where
    I::IntoIter: DoubleEndedIterator + ExactSizeIterator,
{
    b = -b;
    let a = a.into_iter();

    let mut q = vec![F::ZERO; a.len() - 1];

    let mut tmp = F::ZERO;
    for (q, r) in q.iter_mut().rev().zip(a.rev()) {
        let mut lead_coeff = *r;
        lead_coeff.sub_assign(&tmp);
        *q = lead_coeff;
        tmp = lead_coeff;
        tmp.mul_assign(&b);
    }

    q
}

/// This utility function will parallelize an operation that is to be
/// performed over a mutable slice.
pub fn parallelize<T: Send, F: Fn(&mut [T], usize) + Send + Sync + Clone>(v: &mut [T], f: F) {
    // Algorithm rationale:
    //
    // Using the stdlib `chunks_mut` will lead to severe load imbalance.
    // From https://github.com/rust-lang/rust/blob/e94bda3/library/core/src/slice/iter.rs#L1607-L1637
    // if the division is not exact, the last chunk will be the remainder.
    //
    // Dividing 40 items on 12 threads will lead to a chunk size of 40/12 = 3,
    // There will be a 13 chunks of size 3 and 1 of size 1 distributed on 12 threads.
    // This leads to 1 thread working on 6 iterations, 1 on 4 iterations and 10 on 3 iterations,
    // a load imbalance of 2x.
    //
    // Instead we can divide work into chunks of size
    // 4, 4, 4, 4, 3, 3, 3, 3, 3, 3, 3, 3 = 4*4 + 3*8 = 40
    //
    // This would lead to a 6/4 = 1.5x speedup compared to naive chunks_mut
    //
    // See also OpenMP spec (page 60)
    // http://www.openmp.org/mp-documents/openmp-4.5.pdf
    // "When no chunk_size is specified, the iteration space is divided into chunks
    // that are approximately equal in size, and at most one chunk is distributed to
    // each thread. The size of the chunks is unspecified in this case."
    // This implies chunks are the same size ±1

    let f = &f;
    let total_iters = v.len();
    let num_threads = multicore::current_num_threads();
    let base_chunk_size = total_iters / num_threads;
    let cutoff_chunk_id = total_iters % num_threads;
    let split_pos = cutoff_chunk_id * (base_chunk_size + 1);
    let (v_hi, v_lo) = v.split_at_mut(split_pos);

    multicore::scope(|scope| {
        // Skip special-case: number of iterations is cleanly divided by number of threads.
        if cutoff_chunk_id != 0 {
            for (chunk_id, chunk) in v_hi.chunks_exact_mut(base_chunk_size + 1).enumerate() {
                let offset = chunk_id * (base_chunk_size + 1);
                scope.spawn(move |_| f(chunk, offset));
            }
        }
        // Skip special-case: less iterations than number of threads.
        if base_chunk_size != 0 {
            for (chunk_id, chunk) in v_lo.chunks_exact_mut(base_chunk_size).enumerate() {
                let offset = split_pos + (chunk_id * base_chunk_size);
                scope.spawn(move |_| f(chunk, offset));
            }
        }
    });
}

fn log2_floor(num: usize) -> u32 {
    assert!(num > 0);

    let mut pow = 0;

    while (1 << (pow + 1)) <= num {
        pow += 1;
    }

    pow
}

/// Returns coefficients of an n - 1 degree polynomial given a set of n points
/// and their evaluations. This function will panic if two values in `points`
/// are the same.
pub fn lagrange_interpolate<F: Field>(points: &[F], evals: &[F]) -> Vec<F> {
    assert_eq!(points.len(), evals.len());
    if points.len() == 1 {
        // Constant polynomial
        vec![evals[0]]
    } else {
        let mut denoms = Vec::with_capacity(points.len());
        for (j, x_j) in points.iter().enumerate() {
            let mut denom = Vec::with_capacity(points.len() - 1);
            for x_k in points
                .iter()
                .enumerate()
                .filter(|&(k, _)| k != j)
                .map(|a| a.1)
            {
                denom.push(*x_j - x_k);
            }
            denoms.push(denom);
        }
        // Compute (x_j - x_k)^(-1) for each j != i
        denoms.iter_mut().flat_map(|v| v.iter_mut()).batch_invert();

        let mut final_poly = vec![F::ZERO; points.len()];
        for (j, (denoms, eval)) in denoms.into_iter().zip(evals.iter()).enumerate() {
            let mut tmp: Vec<F> = Vec::with_capacity(points.len());
            let mut product = Vec::with_capacity(points.len() - 1);
            tmp.push(F::ONE);
            for (x_k, denom) in points
                .iter()
                .enumerate()
                .filter(|&(k, _)| k != j)
                .map(|a| a.1)
                .zip(denoms.into_iter())
            {
                product.resize(tmp.len() + 1, F::ZERO);
                for ((a, b), product) in tmp
                    .iter()
                    .chain(std::iter::once(&F::ZERO))
                    .zip(std::iter::once(&F::ZERO).chain(tmp.iter()))
                    .zip(product.iter_mut())
                {
                    *product = *a * (-denom * x_k) + *b * denom;
                }
                std::mem::swap(&mut tmp, &mut product);
            }
            assert_eq!(tmp.len(), points.len());
            assert_eq!(product.len(), points.len() - 1);
            for (final_coeff, interpolation_coeff) in final_poly.iter_mut().zip(tmp.into_iter()) {
                *final_coeff += interpolation_coeff * eval;
            }
        }
        final_poly
    }
}

pub(crate) fn evaluate_vanishing_polynomial<F: Field>(roots: &[F], z: F) -> F {
    fn evaluate<F: Field>(roots: &[F], z: F) -> F {
        roots.iter().fold(F::ONE, |acc, point| (z - point) * acc)
    }
    let n = roots.len();
    let num_threads = multicore::current_num_threads();
    if n * 2 < num_threads {
        evaluate(roots, z)
    } else {
        let chunk_size = (n + num_threads - 1) / num_threads;
        let mut parts = vec![F::ONE; num_threads];
        multicore::scope(|scope| {
            for (out, roots) in parts.chunks_mut(1).zip(roots.chunks(chunk_size)) {
                scope.spawn(move |_| out[0] = evaluate(roots, z));
            }
        });
        parts.iter().fold(F::ONE, |acc, part| acc * part)
    }
}

pub(crate) fn powers<F: Field>(base: F) -> impl Iterator<Item = F> {
    std::iter::successors(Some(F::ONE), move |power| Some(base * power))
}

#[cfg(test)]
use rand_core::OsRng;

#[cfg(test)]
use crate::halo2curves::pasta::Fp;

#[test]
fn test_lagrange_interpolate() {
    let rng = OsRng;

    let points = (0..5).map(|_| Fp::random(rng)).collect::<Vec<_>>();
    let evals = (0..5).map(|_| Fp::random(rng)).collect::<Vec<_>>();

    for coeffs in 0..5 {
        let points = &points[0..coeffs];
        let evals = &evals[0..coeffs];

        let poly = lagrange_interpolate(points, evals);
        assert_eq!(poly.len(), points.len());

        for (point, eval) in points.iter().zip(evals) {
            assert_eq!(eval_polynomial(&poly, *point), *eval);
        }
    }
}



#[test]
fn test_compare_cpu_gpu_msm() {
    use halo2curves::bn256::{Bn256, Fr, G1Affine, G1}; // Replace with appropriate curve
    use std::time::Instant;
    use rand_core::OsRng;
    use rand_chacha::ChaChaRng;
    use rand_core::{SeedableRng, RngCore};
    use group::{Curve, prime::PrimeCurveAffine}; // For scalar multiplication and identity functions
    use crate::halo2curves::pairing::Engine;
    use cpu_multiexp;
    use gpu_multiexp;
    
    // Define the range of MSM sizes to test, from 2^10 to 2^16
    let start_exp = 10;
    let end_exp = 15;
    let seed = [0u8; 32]; // You can change this to any 32-byte array
    let mut rng = ChaChaRng::from_seed(seed);
        
    for k in start_exp..=end_exp {
        let num_elements = 1 << k;
        println!("\nTesting with num_elements: {:?}", num_elements);

        // Generate random coefficients (scalars)
        let coeffs: Vec<Fr> = (0..num_elements).map(|_| Fr::random(&mut rng)).collect();

        let mut bases = (0..num_elements)
        .map(|_| G1Affine::random(&mut rng)) // Generate random points for each base
        .collect::<Vec<_>>();
        
        // Run the multi-exponentiation using the best_multiexp_cpu function
        let timer = Instant::now();
        let cpu_result = cpu_multiexp(&coeffs, &bases);
        let cpu_elapsed = timer.elapsed();
        println!("CPU Result: {:?}", cpu_result.to_affine());
        println!("CPU elapsed time: {:?}", cpu_elapsed);

        // Run the multi-exponentiation using the best_multiexp_gpu function
        let timer = Instant::now();
        let gpu_result = gpu_multiexp(&coeffs, &bases).unwrap();
        let gpu_elapsed = timer.elapsed();
        println!("GPU Result: {:?}", gpu_result.to_affine());
        println!("GPU elapsed time: {:?}", gpu_elapsed);

        println!("Speedup: x{}", cpu_elapsed.as_secs_f32() / gpu_elapsed.as_secs_f32());

        assert_eq!(cpu_result.to_affine(), gpu_result.to_affine())
        // Verify that the results match
        // assert_eq!(cpu_result, gpu_result, "MSM result does not match for size {}", num_elements);


        // // Output results for this size
        // println!("num_elements: {}, elapsed time: {:?}, result {:?}", num_elements, elapsed_time, result);

        // // // Optional: Verify the result with a serial MSM implementation
        // let mut expected_result = G1::identity();
        // for (base, coeff) in bases.iter().zip(coeffs.iter()) {
        //     // Convert base from G1Affine to G1 before multiplication.
        //     expected_result +=  G1Affine::from(base * coeff);
        // }
        // assert_eq!(G1Affine::from(result), G1Affine::from(expected_result), "MSM result does not match for size {}", num_elements);
    }
}




#[test]
fn test_compare_cpu_gpu_fft() {
    use crate::poly::EvaluationDomain;
    use std::time::Instant;
    use halo2curves::bn256::Fr;
    use rand_core::OsRng;
    use rand_chacha::ChaChaRng;
    use rand_core::{SeedableRng, RngCore};
    use cpu_fft;
    use gpu_fft;

    let seed = [0u8; 32]; // You can change this to any 32-byte array
    let mut rng = ChaChaRng::from_seed(seed);
    
    for k in 16..=20 {
        // polynomial degree n = 2^k
        let n = 1u64 << k;
        let log_n = k; // log_n is just k because n = 2^k
        
        // polynomial coeffs
        let inital_coeffs: Vec<_> = (0..n).map(|_| Fr::random(&mut rng)).collect();
        
        let mut cpu_coeffs = inital_coeffs.clone();
        let mut gpu_coeffs = inital_coeffs.clone();
        // evaluation domain
        let domain: EvaluationDomain<Fr> = EvaluationDomain::new(1, k);

        println!("Testing FFT for {} elements, degree {}...", n, k);
        
        let timer = Instant::now();
        cpu_fft(&mut cpu_coeffs, domain.get_omega(), k);
        let cpu_dur = timer.elapsed();
        println!("CPU FFT took {:?}", cpu_dur);

        let timer = Instant::now(); // Reset timer
        gpu_fft(&mut gpu_coeffs, domain.get_omega(), k);
        let gpu_dur = timer.elapsed();
        println!("GPU FFT took {:?}", gpu_dur);

        println!("Speedup: x{}", cpu_dur.as_secs_f32() / gpu_dur.as_secs_f32());
        // assert_eq!(cpu_coeffs, inital_coeffs);
        // Allow small relative error
        assert_eq!(cpu_coeffs, gpu_coeffs);
    }
}
