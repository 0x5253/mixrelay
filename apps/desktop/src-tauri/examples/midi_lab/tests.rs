use super::*;

#[test]
fn a_person_can_learn_each_control_individually_before_running_the_script() {
    for number in 60..=64 {
        assert_eq!(command(&format!("n{number}")), Some(Command::Note(number)));
    }
    assert_eq!(command("c7"), Some(Command::Cc(7, 0)));
    assert_eq!(command("c10"), Some(Command::Cc(10, 0)));
    assert_eq!(command("c11"), Some(Command::Cc(11, 64)));
    assert_eq!(command("\n"), Some(Command::Script));
    assert_eq!(command("q"), Some(Command::Quit));
    for invalid in ["n59", "n65", "n256", "nother", "c9", "script"] {
        assert_eq!(command(invalid), None);
    }
}
