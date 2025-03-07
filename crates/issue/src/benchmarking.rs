use super::*;
use bitcoin::{
    formatter::{Formattable, TryFormattable},
    types::{
        BlockBuilder, RawBlockHeader, TransactionBuilder, TransactionInputBuilder, TransactionInputSource,
        TransactionOutput,
    },
};
use btc_relay::{BtcAddress, BtcPublicKey};
use frame_benchmarking::{account, benchmarks, impl_benchmark_test_suite};
use frame_system::RawOrigin;
use orml_traits::MultiCurrency;
use primitives::CurrencyId;
use sp_core::{H160, H256, U256};
use sp_runtime::{
    traits::{One, Zero},
    FixedPointNumber,
};
use sp_std::prelude::*;

// Pallets
use crate::Pallet as Issue;
use btc_relay::Pallet as BtcRelay;
use oracle::Pallet as Oracle;
use security::Pallet as Security;
use vault_registry::Pallet as VaultRegistry;

pub const DEFAULT_TESTING_CURRENCY: CurrencyId = CurrencyId::DOT;

fn dummy_public_key() -> BtcPublicKey {
    BtcPublicKey([
        2, 205, 114, 218, 156, 16, 235, 172, 106, 37, 18, 153, 202, 140, 176, 91, 207, 51, 187, 55, 18, 45, 222, 180,
        119, 54, 243, 97, 173, 150, 161, 169, 230,
    ])
}

fn mint_collateral<T: crate::Config>(account_id: &T::AccountId, amount: Collateral<T>) {
    <orml_tokens::Pallet<T>>::deposit(DEFAULT_TESTING_CURRENCY, account_id, amount).unwrap();
}

fn mine_blocks_until_expiry<T: crate::Config>(request: &DefaultIssueRequest<T>) {
    let period = Issue::<T>::issue_period().max(request.period);
    let expiry_height = BtcRelay::<T>::bitcoin_expiry_height(request.btc_height, period).unwrap();
    mine_blocks::<T>(expiry_height + 100);
}

fn mine_blocks<T: crate::Config>(end_height: u32) {
    let relayer_id: T::AccountId = account("Relayer", 0, 0);
    mint_collateral::<T>(&relayer_id, (1u32 << 31).into());

    let height = 0;
    let block = BlockBuilder::new()
        .with_version(4)
        .with_coinbase(&BtcAddress::P2SH(H160::zero()), 50, 3)
        .with_timestamp(1588813835)
        .mine(U256::from(2).pow(254.into()))
        .unwrap();

    let raw_block_header = RawBlockHeader::from_bytes(&block.header.try_format().unwrap()).unwrap();
    let block_header = BtcRelay::<T>::parse_raw_block_header(&raw_block_header).unwrap();

    Security::<T>::set_active_block_number(1u32.into());
    BtcRelay::<T>::initialize(relayer_id.clone(), block_header, height).unwrap();

    let transaction = TransactionBuilder::new()
        .with_version(2)
        .add_input(
            TransactionInputBuilder::new()
                .with_source(TransactionInputSource::FromOutput(block.transactions[0].hash(), 0))
                .with_script(&[
                    0, 71, 48, 68, 2, 32, 91, 128, 41, 150, 96, 53, 187, 63, 230, 129, 53, 234, 210, 186, 21, 187, 98,
                    38, 255, 112, 30, 27, 228, 29, 132, 140, 155, 62, 123, 216, 232, 168, 2, 32, 72, 126, 179, 207,
                    142, 8, 99, 8, 32, 78, 244, 166, 106, 160, 207, 227, 61, 210, 172, 234, 234, 93, 59, 159, 79, 12,
                    194, 240, 212, 3, 120, 50, 1, 71, 81, 33, 3, 113, 209, 131, 177, 9, 29, 242, 229, 15, 217, 247,
                    165, 78, 111, 80, 79, 50, 200, 117, 80, 30, 233, 210, 167, 133, 175, 62, 253, 134, 127, 212, 51,
                    33, 2, 128, 200, 184, 235, 148, 25, 43, 34, 28, 173, 55, 54, 189, 164, 187, 243, 243, 152, 7, 84,
                    210, 85, 156, 238, 77, 97, 188, 240, 162, 197, 105, 62, 82, 174,
                ])
                .build(),
        )
        .build();

    let mut prev_hash = block.header.hash;
    for _ in 0..end_height {
        let block = BlockBuilder::new()
            .with_previous_hash(prev_hash)
            .with_version(4)
            .with_coinbase(&BtcAddress::P2SH(H160::zero()), 50, 3)
            .with_timestamp(1588813835)
            .add_transaction(transaction.clone())
            .mine(U256::from(2).pow(254.into()))
            .unwrap();
        prev_hash = block.header.hash;

        let raw_block_header = RawBlockHeader::from_bytes(&block.header.try_format().unwrap()).unwrap();
        let block_header = BtcRelay::<T>::parse_raw_block_header(&raw_block_header).unwrap();

        BtcRelay::<T>::store_block_header(&relayer_id, block_header).unwrap();
    }
}

benchmarks! {
    request_issue {
        let origin: T::AccountId = account("Origin", 0, 0);
        let amount = Issue::<T>::issue_btc_dust_value().amount() + 1000u32.into();
        let vault_id: T::AccountId = account("Vault", 0, 0);
        let griefing: u32 = 100;
        let relayer_id: T::AccountId = account("Relayer", 0, 0);

        mint_collateral::<T>(&origin, (1u32 << 31).into());
        mint_collateral::<T>(&vault_id, (1u32 << 31).into());
        mint_collateral::<T>(&relayer_id, (1u32 << 31).into());

        Oracle::<T>::_set_exchange_rate(DEFAULT_TESTING_CURRENCY, <T as currency::Config>::UnsignedFixedPoint::one()).unwrap();
        VaultRegistry::<T>::set_secure_collateral_threshold(DEFAULT_TESTING_CURRENCY, <T as currency::Config>::UnsignedFixedPoint::checked_from_rational(1, 100000).unwrap());// 0.001%
        VaultRegistry::<T>::set_collateral_ceiling(DEFAULT_TESTING_CURRENCY, 1_000_000_000u32.into());
        VaultRegistry::<T>::_register_vault(&vault_id, 100000000u32.into(), dummy_public_key(), T::GetGriefingCollateralCurrencyId::get()).unwrap();

        // initialize relay

        let height = 0;
        let block = BlockBuilder::new()
            .with_version(4)
            .with_coinbase(&BtcAddress::P2SH(H160::zero()), 50, 3)
            .with_timestamp(1588813835)
            .mine(U256::from(2).pow(254.into())).unwrap();
        let block_hash = block.header.hash;
        let raw_block_header = RawBlockHeader::from_bytes(&block.header.try_format().unwrap()).unwrap();
        let block_header = BtcRelay::<T>::parse_raw_block_header(&raw_block_header).unwrap();

        Security::<T>::set_active_block_number(1u32.into());
        BtcRelay::<T>::initialize(relayer_id.clone(), block_header, height).unwrap();

        let vault_btc_address = BtcAddress::P2SH(H160::zero());

        let transaction = TransactionBuilder::new()
        .with_version(2)
        .add_input(
            TransactionInputBuilder::new()
                .with_source(TransactionInputSource::FromOutput(block.transactions[0].hash(), 0))
                .with_script(&[
                    0, 71, 48, 68, 2, 32, 91, 128, 41, 150, 96, 53, 187, 63, 230, 129, 53, 234,
                    210, 186, 21, 187, 98, 38, 255, 112, 30, 27, 228, 29, 132, 140, 155, 62, 123,
                    216, 232, 168, 2, 32, 72, 126, 179, 207, 142, 8, 99, 8, 32, 78, 244, 166, 106,
                    160, 207, 227, 61, 210, 172, 234, 234, 93, 59, 159, 79, 12, 194, 240, 212, 3,
                    120, 50, 1, 71, 81, 33, 3, 113, 209, 131, 177, 9, 29, 242, 229, 15, 217, 247,
                    165, 78, 111, 80, 79, 50, 200, 117, 80, 30, 233, 210, 167, 133, 175, 62, 253,
                    134, 127, 212, 51, 33, 2, 128, 200, 184, 235, 148, 25, 43, 34, 28, 173, 55, 54,
                    189, 164, 187, 243, 243, 152, 7, 84, 210, 85, 156, 238, 77, 97, 188, 240, 162,
                    197, 105, 62, 82, 174,
                ])
                .build(),
        )
        .add_output(TransactionOutput::payment(123123, &vault_btc_address))
        .add_output(TransactionOutput::op_return(0, H256::zero().as_bytes()))
        .build();

        let block = BlockBuilder::new()
            .with_previous_hash(block_hash)
            .with_version(4)
            .with_timestamp(1588813835)
            .add_transaction(transaction)
            .mine(U256::from(2).pow(254.into())).unwrap();

        let raw_block_header = RawBlockHeader::from_bytes(&block.header.try_format().unwrap()).unwrap();
        let block_header = BtcRelay::<T>::parse_raw_block_header(&raw_block_header).unwrap();
        BtcRelay::<T>::store_block_header(&relayer_id, block_header).unwrap();
        Security::<T>::set_active_block_number(Security::<T>::active_block_number() + BtcRelay::<T>::parachain_confirmations());

    }: _(RawOrigin::Signed(origin), amount, vault_id, griefing.into())

    execute_issue {
        let origin: T::AccountId = account("Origin", 0, 0);
        let vault_id: T::AccountId = account("Vault", 0, 0);
        let relayer_id: T::AccountId = account("Relayer", 0, 0);

        mint_collateral::<T>(&origin, (1u32 << 31).into());
        mint_collateral::<T>(&vault_id, (1u32 << 31).into());
        mint_collateral::<T>(&relayer_id, (1u32 << 31).into());

        let vault_btc_address = BtcAddress::P2SH(H160::zero());
        let value: Amount<T> = Amount::new(2u32.into(), T::GetWrappedCurrencyId::get());

        let issue_id = H256::zero();
        let mut issue_request = IssueRequest::default();
        issue_request.requester = origin.clone();
        issue_request.vault = vault_id.clone();
        issue_request.btc_address = vault_btc_address;
        issue_request.amount = value.amount();
        Issue::<T>::insert_issue_request(&issue_id, &issue_request);

        let height = 0;
        let block = BlockBuilder::new()
            .with_version(4)
            .with_coinbase(&vault_btc_address, 50, 3)
            .with_timestamp(1588813835)
            .mine(U256::from(2).pow(254.into())).unwrap();

        let block_hash = block.header.hash;
        let raw_block_header = RawBlockHeader::from_bytes(&block.header.try_format().unwrap()).unwrap();
        let block_header = BtcRelay::<T>::parse_raw_block_header(&raw_block_header).unwrap();

        Security::<T>::set_active_block_number(1u32.into());
        BtcRelay::<T>::initialize(relayer_id.clone(), block_header, height).unwrap();

        let transaction = TransactionBuilder::new()
            .with_version(2)
            .add_input(
                TransactionInputBuilder::new()
                    .with_source(TransactionInputSource::FromOutput(block.transactions[0].hash(), 0))
                    .with_script(&[
                        0, 71, 48, 68, 2, 32, 91, 128, 41, 150, 96, 53, 187, 63, 230, 129, 53, 234,
                        210, 186, 21, 187, 98, 38, 255, 112, 30, 27, 228, 29, 132, 140, 155, 62, 123,
                        216, 232, 168, 2, 32, 72, 126, 179, 207, 142, 8, 99, 8, 32, 78, 244, 166, 106,
                        160, 207, 227, 61, 210, 172, 234, 234, 93, 59, 159, 79, 12, 194, 240, 212, 3,
                        120, 50, 1, 71, 81, 33, 3, 113, 209, 131, 177, 9, 29, 242, 229, 15, 217, 247,
                        165, 78, 111, 80, 79, 50, 200, 117, 80, 30, 233, 210, 167, 133, 175, 62, 253,
                        134, 127, 212, 51, 33, 2, 128, 200, 184, 235, 148, 25, 43, 34, 28, 173, 55, 54,
                        189, 164, 187, 243, 243, 152, 7, 84, 210, 85, 156, 238, 77, 97, 188, 240, 162,
                        197, 105, 62, 82, 174,
                    ])
                    .build(),
            )
            .add_output(TransactionOutput::payment(2u32.into(), &vault_btc_address))
            .add_output(TransactionOutput::op_return(0, H256::zero().as_bytes()))
            .build();

        let block = BlockBuilder::new()
            .with_previous_hash(block_hash)
            .with_version(4)
            .with_coinbase(&vault_btc_address, 50, 4)
            .with_timestamp(1588813835)
            .add_transaction(transaction.clone())
            .mine(U256::from(2).pow(254.into())).unwrap();

        let tx_id = transaction.tx_id();
        let proof = block.merkle_proof(&[tx_id]).unwrap().try_format().unwrap();
        let raw_tx = transaction.format_with(true);

        let raw_block_header = RawBlockHeader::from_bytes(&block.header.try_format().unwrap()).unwrap();
        let block_header = BtcRelay::<T>::parse_raw_block_header(&raw_block_header).unwrap();

        BtcRelay::<T>::store_block_header(&relayer_id, block_header).unwrap();
        Security::<T>::set_active_block_number(Security::<T>::active_block_number() + BtcRelay::<T>::parachain_confirmations());

        VaultRegistry::<T>::set_collateral_ceiling(DEFAULT_TESTING_CURRENCY, 1_000_000_000u32.into());
        VaultRegistry::<T>::set_secure_collateral_threshold(DEFAULT_TESTING_CURRENCY, <T as currency::Config>::UnsignedFixedPoint::checked_from_rational(1, 100000).unwrap());
        Oracle::<T>::_set_exchange_rate(DEFAULT_TESTING_CURRENCY, <T as currency::Config>::UnsignedFixedPoint::one()).unwrap();
        VaultRegistry::<T>::_register_vault(&vault_id, 100000000u32.into(), dummy_public_key(), T::GetGriefingCollateralCurrencyId::get()).unwrap();

        VaultRegistry::<T>::try_increase_to_be_issued_tokens(&vault_id, &value).unwrap();
        let secure_id = Security::<T>::get_secure_id(&vault_id);
        VaultRegistry::<T>::register_deposit_address(&vault_id, secure_id).unwrap();
    }: _(RawOrigin::Signed(origin), issue_id, proof, raw_tx)

    cancel_issue {
        let origin: T::AccountId = account("Origin", 0, 0);
        let vault_id: T::AccountId = account("Vault", 0, 0);

        mint_collateral::<T>(&origin, (1u32 << 31).into());
        mint_collateral::<T>(&vault_id, (1u32 << 31).into());

        let vault_btc_address = BtcAddress::P2SH(H160::zero());
        let value = Amount::new(2u32.into(), T::GetWrappedCurrencyId::get());

        let issue_id = H256::zero();
        let mut issue_request = IssueRequest::default();
        issue_request.requester = origin.clone();
        issue_request.vault = vault_id.clone();
        issue_request.btc_address = vault_btc_address;
        issue_request.amount = value.amount();
        issue_request.opentime = Security::<T>::active_block_number();
        issue_request.btc_height = Zero::zero();
        Issue::<T>::insert_issue_request(&issue_id, &issue_request);

        // expire issue request
        mine_blocks_until_expiry::<T>(&issue_request);
        Security::<T>::set_active_block_number(issue_request.opentime + Issue::<T>::issue_period() + 100u32.into());

        VaultRegistry::<T>::set_collateral_ceiling(DEFAULT_TESTING_CURRENCY, 1_000_000_000u32.into());
        VaultRegistry::<T>::set_secure_collateral_threshold(DEFAULT_TESTING_CURRENCY, <T as currency::Config>::UnsignedFixedPoint::checked_from_rational(1, 100000).unwrap());
        Oracle::<T>::_set_exchange_rate(DEFAULT_TESTING_CURRENCY, <T as currency::Config>::UnsignedFixedPoint::one()).unwrap();
        VaultRegistry::<T>::_register_vault(&vault_id, 100000000u32.into(), dummy_public_key(), T::GetGriefingCollateralCurrencyId::get()).unwrap();

        VaultRegistry::<T>::try_increase_to_be_issued_tokens(&vault_id, &value).unwrap();
        let secure_id = Security::<T>::get_secure_id(&vault_id);
        VaultRegistry::<T>::register_deposit_address(&vault_id, secure_id).unwrap();

    }: _(RawOrigin::Signed(origin), issue_id)

    set_issue_period {
    }: _(RawOrigin::Root, 1u32.into())

}

impl_benchmark_test_suite!(
    Issue,
    crate::mock::ExtBuilder::build_with(Default::default()),
    crate::mock::Test
);
