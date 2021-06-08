use crate::{ext, mock::*, Event};
use bitcoin::types::{LockTime, Transaction};
use btc_relay::BtcAddress;
use frame_support::assert_ok;
use mocktopus::mocking::*;
use sp_core::{H160, H256};

fn dummy_transaction() -> Transaction {
    Transaction {
        inputs: vec![],
        outputs: vec![],
        lock_at: LockTime::BlockHeight(1),
        version: 0,
    }
}

#[test]
fn test_refund_succeeds() {
    run_test(|| {
        ext::fee::get_refund_fee_from_total::<Test>.mock_safe(|_| MockResult::Return(Ok(5)));
        ext::vault_registry::try_increase_to_be_issued_tokens::<Test>.mock_safe(|_, _| MockResult::Return(Ok(())));
        ext::vault_registry::issue_tokens::<Test>.mock_safe(|_, _| MockResult::Return(Ok(())));
        ext::btc_relay::parse_transaction::<Test>.mock_safe(|_| MockResult::Return(Ok(dummy_transaction())));
        ext::btc_relay::verify_and_validate_op_return_transaction::<Test, usize>
            .mock_safe(|_, _, _, _, _| MockResult::Return(Ok(())));

        let issue_id = H256::zero();
        assert_ok!(Refund::request_refund(
            1000,
            VAULT,
            USER,
            BtcAddress::P2SH(H160::zero()),
            issue_id
        ));

        // check the emitted event
        let captured_event = System::events()
            .iter()
            .find_map(|x| match &x.event {
                TestEvent::refund(ref event) => Some(event.clone()),
                _ => None,
            })
            .unwrap();
        let refund_id = match captured_event {
            Event::<Test>::RequestRefund(refund_id, issuer, 995, vault, _btc_address, issue, 5)
                if issuer == USER && vault == VAULT && issue == issue_id =>
            {
                Some(refund_id)
            }
            _ => None,
        }
        .unwrap();

        assert_ok!(Refund::_execute_refund(refund_id, vec![0u8; 100], vec![0u8; 100],));
    })
}
