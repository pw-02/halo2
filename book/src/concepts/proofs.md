# Proof systems

The aim of any ***proof system*** is to be able to prove interesting mathematical or
cryptographic ***statements***.

Typically, in a given protocol we will want to prove families of statements that differ
in their ***public inputs***. The prover will also need to show that they know some
***private inputs*** that make the statement hold.

To do this we write down a ***relation***, $\mathcal{R}$, that specifies which
combinations of public and private inputs are valid.

> The terminology above is intended to be aligned with the
> [ZKProof Community Reference](https://docs.zkproof.org/reference#latest-version).

To be precise, we should distinguish between the relation $\mathcal{R}$, and its
implementation to be used in a proof system. We call the latter a ***circuit***.

The language that we use to express circuits for a particular proof system is called an
***arithmetization***. Usually, an arithmetization will define circuits in terms of
polynomial constraints on variables over a field.

> The _process_ of expressing a particular relation as a circuit is also sometimes called
> "arithmetization", but we'll avoid that usage.

To create a proof of a statement, the prover will need to know the private inputs,
and also intermediate values, called ***advice*** values, that are used by the circuit.

We assume that we can compute advice values efficiently from the private and public inputs.
The particular advice values will depend on how we write the circuit, not only on the
high-level statement.

The private inputs and advice values are collectively called a ***witness***.

> Some authors use "witness" as just a synonym for private inputs. But in our usage,
> a witness includes advice, i.e. it includes all values that the prover supplies to
> the circuit.

For example, suppose that we want to prove knowledge of a preimage $x$ of a
hash function $H$ for a digest $y$:

* The private input would be the preimage $x$.

* The public input would be the digest $y$.

* The relation would be $\{(x, y) : H(x) = y\}$.

* For a particular public input $Y$, the statement would be: $\{(x) : H(x) = Y\}$.

* The advice would be all of the intermediate values in the circuit implementing the
  hash function. The witness would be $x$ and the advice.

A ***Non-interactive Argument*** allows a ***prover*** to create a ***proof*** for a
given statement and witness. The proof is data that can be used to convince a ***verifier***
that _there exists_ a witness for which the statement holds. The security property that
such proofs cannot falsely convince a verifier is called ***soundness***.

A ***Non-interactive Argument of Knowledge*** (***NARK***) further convinces the verifier
that the prover _knew_ a witness for which the statement holds. This security property is
called ***knowledge soundness***, and it implies soundness.

In practice knowledge soundness is more useful for cryptographic protocols than soundness:
if we are interested in whether Alice holds a secret key in some protocol, say, we need
Alice to prove that _she knows_ the key, not just that it exists.

Knowledge soundness is formalized by saying that an ***extractor***, which can observe
precisely how the proof is generated, must be able to compute the witness.

> This property is subtle given that proofs can be ***malleable***. That is, depending on the
> proof system it may be possible to take an existing proof (or set of proofs) and, without
> knowing the witness(es), modify it/them to produce a distinct proof of the same or a related
> statement. Higher-level protocols that use malleable proof systems need to take this into
> account.
>
> Even without malleability, proofs can also potentially be ***replayed***. For instance,
> we would not want Alice in our example to be able to present a proof generated by someone
> else, and have that be taken as a demonstration that she knew the key.

If a proof yields no information about the witness (other than that a witness exists and was
known to the prover), then we say that the proof system is ***zero knowledge***.

If a proof system produces short proofs —i.e. of length polylogarithmic in the circuit
size— then we say that it is ***succinct***. A succinct NARK is called a ***SNARK***
(***Succinct Non-Interactive Argument of Knowledge***).

> By this definition, a SNARK need not have verification time polylogarithmic in the circuit
> size. Some papers use the term ***efficient*** to describe a SNARK with that property, but
> we'll avoid that term since it's ambiguous for SNARKs that support amortized or recursive
> verification, which we'll get to later.

A ***zk-SNARK*** is a zero-knowledge SNARK.
