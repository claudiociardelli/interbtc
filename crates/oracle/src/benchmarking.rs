use super::{Pallet as Oracle, *};
use crate::{CurrencyId, OracleKey};
use frame_benchmarking::{account, benchmarks, impl_benchmark_test_suite};
use frame_system::RawOrigin;
use sp_runtime::FixedPointNumber;
use sp_std::prelude::*;

benchmarks! {
    feed_values {
        let u in 1 .. 1000u32;

        let origin: T::AccountId = account("origin", 0, 0);
        <AuthorizedOracles<T>>::insert(origin.clone(), Vec::<u8>::new());

        let key = OracleKey::ExchangeRate(CurrencyId::DOT);
        let values:Vec<_> = (0 .. u).map(|x| (key.clone(), UnsignedFixedPoint::<T>::checked_from_rational(1, x+1).unwrap())).collect();
    }: _(RawOrigin::Signed(origin), values)
    verify {
        let key = OracleKey::ExchangeRate(CurrencyId::DOT);
        crate::Pallet::<T>::begin_block(0u32.into());
        assert!(Aggregate::<T>::get(key).is_some());
    }

    insert_authorized_oracle {
        let origin: T::AccountId = account("origin", 0, 0);
    }: _(RawOrigin::Root, origin.clone(), "Origin".as_bytes().to_vec())
    verify {
        assert_eq!(Oracle::<T>::is_authorized(&origin), true);
    }

    remove_authorized_oracle {
        let origin: T::AccountId = account("origin", 0, 0);
        Oracle::<T>::insert_oracle(origin.clone(), "Origin".as_bytes().to_vec());
    }: _(RawOrigin::Root, origin.clone())
    verify {
        assert_eq!(Oracle::<T>::is_authorized(&origin), false);
    }
}

impl_benchmark_test_suite!(Oracle, crate::mock::ExtBuilder::build(), crate::mock::Test);
