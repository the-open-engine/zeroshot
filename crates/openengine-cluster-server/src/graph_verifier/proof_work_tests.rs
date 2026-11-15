use super::*;

#[test]
fn a_failed_charge_permanently_exhausts_the_admission_budget() {
    let budget = ProofWork::default();
    assert!(budget.charge(1).is_some());
    assert!(budget.charge(ProofWork::LIMIT as usize).is_none());
    assert!(budget.charge(1).is_none());
    assert!(budget.charge(0).is_none());
    let exact = ProofWork::default();
    assert!(exact.charge(ProofWork::LIMIT as usize).is_some());
    assert!(exact.charge(0).is_some());
    assert!(exact.charge(1).is_none());
}
