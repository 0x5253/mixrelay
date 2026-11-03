//! Gate, expansion and sidechain-ducking contracts.
use super::*;

#[test]
fn ducking_requires_control_input_and_recovers_without_it() {
    let config = configs().remove(9);
    let input = vec![0.5; 9600];
    assert_eq!(signal(&config, &input, None, &[128]), input);
    let mut control = vec![1.0; 4800];
    control[1000..].fill(0.0);
    let output = signal(&config, &input, Some(&control), &[31]);
    assert!((output[1000] - 0.5 * 10.0_f32.powf(-12.0 / 20.0)).abs() < 1e-5);
    assert!(output[9500] > 0.499);
    assert!(output.chunks_exact(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn gate_and_expander_attenuate_quiet_signal_and_open_for_loud_signal() {
    for index in [3, 4] {
        let config = &configs()[index];
        let quiet = signal(config, &vec![0.001; 9600], None, &[64]);
        assert!(quiet[9500] < 0.0001);
        let loud = signal(config, &vec![0.5; 9600], None, &[64]);
        assert!(loud[9500] > 0.499);
    }
}
