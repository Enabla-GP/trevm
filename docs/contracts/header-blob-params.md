# Explicit header blob parameters (ENA-2773)

The `0.43.1-enabla.1` fork API replaces the `Block` implementations for bare
Alloy consensus headers, RPC headers and RPC blocks. `HeaderWithBlobParams`
requires the caller to supply the blob parameters already resolved from the
block's chain specification and timestamp. There is no fallback or schedule
selection based on header fields, the EVM spec id, or a default network.

The caller remains responsible for selecting and validating its chain
specification and rejecting unsupported execution eras and invalid headers.
The fork does not duplicate a chain resolver. BPO1 and BPO2 do not introduce
header fields that identify their activation. Two chains may require different
parameters for identical header metadata.

```rust
use alloy::{consensus::Header, eips::eip7840::BlobParams};
use trevm::{Block, HeaderWithBlobParams};

let header = Header { excess_blob_gas: Some(30_000_000), ..Default::default() };
// This value represents the output of the caller's chain resolver.
let resolved = BlobParams::bpo2();
let filler = HeaderWithBlobParams::new(&header, resolved);
let env = filler.to_block_env();
assert_eq!(env.blob_excess_gas_and_price.unwrap().blob_gasprice, 13);
```

Replace `fill_block(&header)` with `fill_block(&filler)`. For an RPC header,
construct the filler with `&rpc_header.inner`. For an RPC block, use
`HeaderWithBlobParams::from_rpc_block(&rpc_block, resolved)` to retain the
transaction-count hint. Generic user-provided `Block` implementations and
`BlockEnv` fillers keep their existing interface.

The filler computes the current block's blob fee using the supplied
`BlobParams::calc_blob_fee`, preserving its minimum fee and `u128` update
fraction. The fraction must be nonzero; construction rejects zero with a
documented panic. A header without excess blob gas clears any previous blob
gas/price state. The supplied parameters are unused for that blobless header;
this does not validate whether omission is permitted at the caller's era.

The fork owns this input boundary and migration of its concrete callers.
ENA-2846 owns the shared execution/validation chain resolver, root/edge caller
migration, and root/edge pin/version cutover to the published fork revision.
No consumer may report affected-fork receipt parity until it uses that corrected
revision and its required acceptance evidence passes.

Protocol references:

- [EIP-7892](https://eips.ethereum.org/EIPS/eip-7892): current-block schedule input.
- [EIP-7691](https://eips.ethereum.org/EIPS/eip-7691): Prague update fraction.
- [EIP-8134](https://eips.ethereum.org/EIPS/eip-8134): BPO1 parameters and mainnet activation.
- [EIP-8135](https://eips.ethereum.org/EIPS/eip-8135): BPO2 parameters and mainnet activation.
