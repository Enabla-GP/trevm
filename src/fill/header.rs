use crate::Block;
use alloy::{
    consensus::Header, eips::eip7840::BlobParams, primitives::U256,
    rpc::types::eth::Block as RpcBlock,
};
use revm::{context::BlockEnv, context_interface::block::BlobExcessGasAndPrice};

/// An Alloy header paired with blob parameters resolved from its chain specification.
///
/// A header does not identify its chain's blob schedule. Resolve the parameters
/// for the header's timestamp before constructing this filler, including BPO
/// updates. The filler never selects parameters from header field presence.
/// For a header without excess blob gas, the parameters are unused and any
/// previous blob environment is cleared.
///
/// ```
/// use alloy::{consensus::Header, eips::eip7840::BlobParams};
/// use trevm::{Block, HeaderWithBlobParams};
///
/// let header = Header { excess_blob_gas: Some(30_000_000), ..Default::default() };
/// // Supply the output of the caller's chain-specification resolver.
/// let resolved_blob_params = BlobParams::bpo2();
/// let block = HeaderWithBlobParams::new(&header, resolved_blob_params).to_block_env();
/// assert_eq!(block.blob_excess_gas_and_price.unwrap().blob_gasprice, 13);
/// ```
///
/// Bare headers cannot silently choose a blob schedule:
///
/// ```compile_fail
/// use alloy::consensus::Header;
/// use trevm::Block;
/// let _ = Header::default().to_block_env();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct HeaderWithBlobParams<'a> {
    header: &'a Header,
    blob_params: BlobParams,
    tx_count_hint: Option<usize>,
}

impl<'a> HeaderWithBlobParams<'a> {
    /// Pair a consensus header with its already resolved blob parameters.
    ///
    /// RPC headers use `HeaderWithBlobParams::new(&rpc_header.inner, params)`.
    /// Headers without excess blob gas ignore the supplied parameters.
    ///
    /// # Panics
    ///
    /// Panics if the update fraction is zero. Chain configuration must supply a
    /// valid, nonzero fraction before an EVM block environment can be filled.
    pub const fn new(header: &'a Header, blob_params: BlobParams) -> Self {
        assert!(blob_params.update_fraction != 0, "blob update fraction must be nonzero");
        Self { header, blob_params, tx_count_hint: None }
    }

    /// Pair an RPC block's header with its already resolved blob parameters.
    ///
    /// Retains the RPC block's transaction count for execution pre-allocation.
    /// The payload representation does not affect the blob calculation.
    ///
    /// # Panics
    ///
    /// Panics if the update fraction is zero, as with [`Self::new`].
    pub const fn from_rpc_block<T>(block: &'a RpcBlock<T>, blob_params: BlobParams) -> Self {
        let mut filler = Self::new(&block.header.inner, blob_params);
        filler.tx_count_hint = Some(block.transactions.len());
        filler
    }
}

impl Block for HeaderWithBlobParams<'_> {
    fn fill_block_env(&self, block_env: &mut BlockEnv) {
        let BlockEnv {
            number,
            beneficiary,
            timestamp,
            gas_limit,
            basefee,
            difficulty,
            prevrandao,
            blob_excess_gas_and_price,
            slot_num: _,
        } = block_env;
        let header = self.header;
        *number = U256::from(header.number);
        *beneficiary = header.beneficiary;
        *timestamp = U256::from(header.timestamp);
        *gas_limit = header.gas_limit;
        *basefee = header.base_fee_per_gas.unwrap_or_default();
        *difficulty = header.difficulty;
        *prevrandao = Some(header.mix_hash);
        *blob_excess_gas_and_price =
            header.excess_blob_gas.map(|excess_blob_gas| BlobExcessGasAndPrice {
                excess_blob_gas,
                blob_gasprice: self.blob_params.calc_blob_fee(excess_blob_gas),
            });
    }

    fn tx_count_hint(&self) -> Option<usize> {
        self.tx_count_hint
    }
}

#[cfg(test)]
mod tests {
    use super::HeaderWithBlobParams;
    use crate::Block;
    use alloy::{
        consensus::Header,
        eips::eip7840::BlobParams,
        primitives::{B256, U256},
        rpc::types::eth::{Block as RpcBlock, BlockTransactions},
    };
    use revm::{context::BlockEnv, context_interface::block::BlobExcessGasAndPrice};

    // Independent EIP-4844 integer-series oracle; fractions below are literal
    // protocol inputs, not values read back from the production BlobParams.
    fn reference_blob_fee(excess_blob_gas: u64, fraction: u128) -> u128 {
        let mut accum = fraction;
        let mut output = 0;
        let mut i = 1;
        while accum != 0 {
            output += accum;
            accum = accum * u128::from(excess_blob_gas) / (fraction * i);
            i += 1;
        }
        output / fraction
    }

    #[test]
    fn header_fill_uses_explicit_prague_bpo1_bpo2_parameters() {
        // EIP-7600, EIP-8134 and EIP-8135 supply mainnet timestamps.
        // EIP-7691 / EIP-7892 supply the literal update fractions.
        let cases = [
            (1_746_612_311, BlobParams::prague(), 5_007_716, 399),
            (1_765_290_071, BlobParams::bpo1(), 8_346_193, 36),
            (1_767_747_671, BlobParams::bpo2(), 11_684_671, 13),
        ];
        for (timestamp, params, fraction, expected) in cases {
            let header =
                Header { timestamp, excess_blob_gas: Some(30_000_000), ..Default::default() };
            assert_eq!(reference_blob_fee(30_000_000, fraction), expected);
            let block = HeaderWithBlobParams::new(&header, params).to_block_env();
            let blob = block.blob_excess_gas_and_price.unwrap();
            assert_eq!(blob.excess_blob_gas, 30_000_000);
            assert_eq!(blob.blob_gasprice, expected);
            assert_eq!(block.timestamp, U256::from(timestamp));
        }
    }

    #[test]
    fn identical_headers_use_the_supplied_chain_parameters() {
        let header = Header {
            timestamp: 1_767_747_671,
            excess_blob_gas: Some(30_000_000),
            ..Default::default()
        };
        let prague = HeaderWithBlobParams::new(&header, BlobParams::prague()).to_block_env();
        let bpo2 = HeaderWithBlobParams::new(&header, BlobParams::bpo2()).to_block_env();
        let cancun = HeaderWithBlobParams::new(&header, BlobParams::cancun()).to_block_env();
        assert_eq!(prague.blob_excess_gas_and_price.unwrap().blob_gasprice, 399);
        assert_eq!(bpo2.blob_excess_gas_and_price.unwrap().blob_gasprice, 13);
        assert_eq!(cancun.blob_excess_gas_and_price.unwrap().blob_gasprice, 7_991);
    }

    #[test]
    fn blobless_header_clears_previous_blob_state() {
        let mut block = BlockEnv {
            blob_excess_gas_and_price: Some(BlobExcessGasAndPrice {
                excess_blob_gas: 30_000_000,
                blob_gasprice: 13,
            }),
            ..Default::default()
        };
        HeaderWithBlobParams::new(&Header::default(), BlobParams::bpo2())
            .fill_block_env(&mut block);
        assert_eq!(block.blob_excess_gas_and_price, None);
    }

    #[test]
    fn custom_blob_parameters_are_preserved_without_fraction_narrowing() {
        let params = BlobParams {
            min_blob_fee: 7,
            update_fraction: u128::from(u64::MAX) + 1,
            ..BlobParams::bpo2()
        };
        let header = Header { excess_blob_gas: Some(0), ..Default::default() };
        let block = HeaderWithBlobParams::new(&header, params).to_block_env();
        assert_eq!(block.blob_excess_gas_and_price.unwrap().blob_gasprice, 7);
    }

    #[test]
    fn rpc_block_preserves_blob_parameters_and_transaction_count() {
        let mut rpc = RpcBlock::<u8>::default();
        rpc.header.inner.excess_blob_gas = Some(30_000_000);
        rpc.transactions =
            BlockTransactions::Hashes(vec![B256::repeat_byte(1), B256::repeat_byte(2)]);
        let filler = HeaderWithBlobParams::from_rpc_block(&rpc, BlobParams::bpo1());
        assert_eq!(filler.tx_count_hint(), Some(2));
        assert_eq!(filler.to_block_env().blob_excess_gas_and_price.unwrap().blob_gasprice, 36);
    }

    #[test]
    #[should_panic(expected = "blob update fraction must be nonzero")]
    fn zero_update_fraction_is_rejected_at_construction() {
        let params = BlobParams { update_fraction: 0, ..BlobParams::bpo2() };
        let _ = HeaderWithBlobParams::new(&Header::default(), params);
    }
}
