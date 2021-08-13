mod mock;

use mock::*;

pub const USER: [u8; 32] = ALICE;
pub const VAULT: [u8; 32] = BOB;

fn test_with<R>(execute: impl Fn(CurrencyId) -> R) {
    let test_with = |currency_id| {
        ExtBuilder::build().execute_with(|| {
            assert_ok!(ExchangeRateOraclePallet::_set_exchange_rate(
                currency_id,
                FixedU128::one()
            ));
            UserData::force_to(USER, default_user_state());
            CoreVaultData::force_to(VAULT, default_vault_state(currency_id));
            execute(currency_id)
        })
    };
    test_with(CurrencyId::DOT);
    test_with(CurrencyId::KSM);
}

mod deposit_collateral_test {
    use super::*;

    #[test]
    fn integration_test_vault_registry_deposit_collateral_below_capacity_succeeds() {
        test_with(|currency_id| {
            let amount = 1_000;

            assert_ok!(Call::VaultRegistry(VaultRegistryCall::deposit_collateral(amount))
                .dispatch(origin_of(account_of(VAULT))));

            assert_eq!(
                ParachainState::get(),
                ParachainState::get_default(currency_id).with_changes(|_, vault, _, _| {
                    vault.backing_collateral += amount;
                    *vault.free_balance.get_mut(&vault.collateral_currency).unwrap() -= amount;
                })
            );
        });
    }

    #[test]
    fn integration_test_vault_registry_lock_additional_at_capacity_succeeds() {
        test_with(|currency_id| {
            let amount = DEFAULT_VAULT_FREE_BALANCE;

            assert_ok!(Call::VaultRegistry(VaultRegistryCall::deposit_collateral(amount))
                .dispatch(origin_of(account_of(VAULT))));

            assert_eq!(
                ParachainState::get(),
                ParachainState::get_default(currency_id).with_changes(|_, vault, _, _| {
                    vault.backing_collateral += amount;
                    *vault.free_balance.get_mut(&vault.collateral_currency).unwrap() -= amount;
                })
            );
        });
    }

    #[test]
    fn integration_test_vault_registry_lock_additional_above_capacity_fails() {
        test_with(|_currency_id| {
            let amount = DEFAULT_VAULT_FREE_BALANCE + 1;

            assert_noop!(
                Call::VaultRegistry(VaultRegistryCall::deposit_collateral(amount))
                    .dispatch(origin_of(account_of(VAULT))),
                TokensError::BalanceTooLow
            );
        });
    }
}
mod withdraw_collateral_test {
    use super::*;

    fn required_collateral() -> u128 {
        VaultRegistryPallet::get_required_collateral_for_vault(account_of(VAULT)).unwrap()
    }

    #[test]
    fn integration_test_vault_registry_withdraw_collateral_below_capacity_succeeds() {
        test_with(|currency_id| {
            let amount = 1_000;

            assert_ok!(Call::VaultRegistry(VaultRegistryCall::withdraw_collateral(amount))
                .dispatch(origin_of(account_of(VAULT))));

            assert_eq!(
                ParachainState::get(),
                ParachainState::get_default(currency_id).with_changes(|_, vault, _, _| {
                    vault.backing_collateral -= amount;
                    *vault.free_balance.get_mut(&vault.collateral_currency).unwrap() += amount;
                })
            );
        });
    }

    #[test]
    fn integration_test_vault_registry_lock_additional_at_capacity_succeeds() {
        test_with(|currency_id| {
            let amount = DEFAULT_VAULT_BACKING_COLLATERAL - required_collateral();

            assert_ok!(Call::VaultRegistry(VaultRegistryCall::withdraw_collateral(amount))
                .dispatch(origin_of(account_of(VAULT))));

            assert_eq!(
                ParachainState::get(),
                ParachainState::get_default(currency_id).with_changes(|_, vault, _, _| {
                    vault.backing_collateral -= amount;
                    *vault.free_balance.get_mut(&vault.collateral_currency).unwrap() += amount;
                })
            );
        });
    }

    #[test]
    fn integration_test_vault_registry_lock_additional_above_capacity_fails() {
        test_with(|_currency_id| {
            let amount = DEFAULT_VAULT_BACKING_COLLATERAL - required_collateral() + 1;

            assert_noop!(
                Call::VaultRegistry(VaultRegistryCall::withdraw_collateral(amount))
                    .dispatch(origin_of(account_of(VAULT))),
                VaultRegistryError::InsufficientCollateral
            );
        });
    }
}

#[test]
fn integration_test_vault_registry_with_parachain_shutdown_fails() {
    test_with(|currency_id| {
        SecurityPallet::set_status(StatusCode::Shutdown);

        assert_noop!(
            Call::VaultRegistry(VaultRegistryCall::register_vault(0, Default::default(), currency_id))
                .dispatch(origin_of(account_of(VAULT))),
            SecurityError::ParachainShutdown
        );
        assert_noop!(
            Call::VaultRegistry(VaultRegistryCall::deposit_collateral(0)).dispatch(origin_of(account_of(VAULT))),
            SecurityError::ParachainShutdown
        );
        assert_noop!(
            Call::VaultRegistry(VaultRegistryCall::withdraw_collateral(0)).dispatch(origin_of(account_of(VAULT))),
            SecurityError::ParachainShutdown
        );
        assert_noop!(
            Call::VaultRegistry(VaultRegistryCall::update_public_key(Default::default()))
                .dispatch(origin_of(account_of(VAULT))),
            SecurityError::ParachainShutdown
        );
        assert_noop!(
            Call::VaultRegistry(VaultRegistryCall::register_address(Default::default()))
                .dispatch(origin_of(account_of(VAULT))),
            SecurityError::ParachainShutdown
        );
        assert_noop!(
            Call::VaultRegistry(VaultRegistryCall::accept_new_issues(false)).dispatch(origin_of(account_of(VAULT))),
            SecurityError::ParachainShutdown
        );
    });
}

#[test]
fn integration_test_vault_registry_undercollateralization_liquidation() {
    test_with(|currency_id| {
        liquidate_vault(currency_id, VAULT);

        assert_eq!(
            ParachainState::get(),
            ParachainState::get_default(currency_id).with_changes(|_, vault, liquidation_vault, _| {
                *liquidation_vault.funds.get_mut(&currency_id).unwrap() = (DEFAULT_VAULT_BACKING_COLLATERAL
                    * (DEFAULT_VAULT_ISSUED + DEFAULT_VAULT_TO_BE_ISSUED - DEFAULT_VAULT_TO_BE_REDEEMED))
                    / (DEFAULT_VAULT_ISSUED + DEFAULT_VAULT_TO_BE_ISSUED);
                liquidation_vault.to_be_issued = DEFAULT_VAULT_TO_BE_ISSUED;
                liquidation_vault.issued = DEFAULT_VAULT_ISSUED;
                liquidation_vault.to_be_redeemed = DEFAULT_VAULT_TO_BE_REDEEMED;

                vault.issued = 0;
                vault.to_be_issued = 0;
                vault.backing_collateral = 0;
                vault.liquidated_collateral = DEFAULT_VAULT_BACKING_COLLATERAL - liquidation_vault.funds[&currency_id];
            })
        );
    });
}
