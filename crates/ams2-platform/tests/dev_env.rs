//! `STINTMATE_DEV_INSTANCE` end to end. One test only: it changes the process
//! environment, so it must not share a process with other tests.
#![cfg(windows)]

use ams2_platform::dev;
use ams2_platform::single::Names;

#[test]
fn the_environment_variable_isolates_names_and_pipes() {
    // SAFETY: the only test of this binary, so no other thread reads the environment.
    unsafe { std::env::remove_var(dev::ENV) };
    assert_eq!(Names::default(), Names::production());
    let real = dev::pipe_path("pitwall").unwrap();
    assert!(dev::pipe_path_if_dev("pitwall").is_none());

    unsafe { std::env::set_var(dev::ENV, "Desk 1") };
    assert_eq!(dev::tag().as_deref(), Some("desk-1"));
    assert_eq!(Names::default(), Names::tagged("desk-1"));
    let tagged = dev::pipe_path("pitwall").unwrap();
    assert_eq!(tagged, format!("{real}-desk-1"));
    assert_eq!(dev::pipe_path_if_dev("pitwall"), Some(tagged));

    unsafe { std::env::set_var(dev::ENV, "  ") };
    assert_eq!(Names::default(), Names::production());
    assert_eq!(dev::pipe_path("pitwall").unwrap(), real);
}
