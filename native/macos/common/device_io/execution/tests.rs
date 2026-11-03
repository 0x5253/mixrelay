use super::*;

#[test]
fn a_clock_break_sees_the_previous_complete_callback_and_current_entry() {
    let execution = Execution::default();
    {
        let _scope = execution.enter(1_000, || 1_150);
        execution.observe_clock();
        assert_eq!(execution.coordinates(), (0, 0, 1_000));
    }
    let _scope = execution.enter(21_000, || 21_070);
    execution.observe_clock();
    assert_eq!(execution.coordinates(), (1_000, 1_150, 21_000));
    // The 19,850-tick gap is distinct from the prior 150-tick execution.
}

#[test]
fn early_returns_without_a_native_packet_do_not_relabel_its_execution() {
    let execution = Execution::default();
    {
        let _scope = execution.enter(10, || 20);
        execution.observe_clock();
    }
    {
        let _scope = execution.enter(30, || {
            panic!("Completion clock read without a native packet")
        });
    }
    let _scope = execution.enter(50, || 60);
    assert_eq!(execution.coordinates(), (10, 20, 50));
}

#[test]
fn a_drained_reconnect_cannot_attribute_an_old_registration_to_new_audio() {
    let execution = Execution::default();
    {
        let _scope = execution.enter(10, || 20);
        execution.observe_clock();
    }
    execution.reset();
    let _scope = execution.enter(100, || 110);
    assert_eq!(execution.coordinates(), (0, 0, 100));
}
