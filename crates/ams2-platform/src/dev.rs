//! Development isolation: a second connector that shares nothing with the installed one.
//!
//! `AMS2_CONNECTOR_DEV_INSTANCE=<tag>` is read here and nowhere else. When set, the
//! single-instance names ([`crate::single::Names::default`]) carry the tag and every pipe the
//! three executables build uses the pipe user `<user>-<tag>`, so the connector's own endpoint
//! and the Pitwall pipe it dials are a separate set from the real ones. For development and
//! desk tests only (see `docs/usage.md`); production never sets it.

/// The environment variable.
pub const ENV: &str = "AMS2_CONNECTOR_DEV_INSTANCE";

/// Longest tag kept.
const MAX_TAG: usize = 24;

/// Normalises a raw tag: ASCII-lowercased, anything but `a-z0-9` becomes `-`, at most 24
/// characters; `None` when nothing is left (unset, empty or blank).
pub fn parse_tag(raw: Option<&str>) -> Option<String> {
    let t: String = raw?
        .trim()
        .chars()
        .take(MAX_TAG)
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let t = t.trim_matches('-').to_owned();
    (!t.is_empty()).then_some(t)
}

/// The development tag of this process, from [`ENV`].
pub fn tag() -> Option<String> {
    parse_tag(std::env::var(ENV).ok().as_deref())
}

/// The pipe user for `user` under `tag`: `<user>-<tag>`.
pub fn tagged_user(user: &str, tag: &str) -> String {
    format!("{user}-{tag}")
}

/// `\\.\pipe\stintmate-<app>-<user>[-<tag>]` for an explicit user and tag.
pub fn pipe_path_with(
    app: &str,
    user: &str,
    tag: Option<&str>,
) -> Result<String, stintmate_ipc::error::NameError> {
    match tag {
        Some(t) => stintmate_ipc::naming::pipe_path_for(app, None, &tagged_user(user, t)),
        None => stintmate_ipc::naming::pipe_path_for(app, None, user),
    }
}

/// The pipe path of endpoint `app` for this process: the user's real one, or the tagged one
/// when [`ENV`] is set. Every pipe of the three executables is built here.
pub fn pipe_path(app: &str) -> Result<String, stintmate_ipc::error::NameError> {
    match tag() {
        None => stintmate_ipc::naming::pipe_path(app, None),
        Some(t) => pipe_path_with(app, &stintmate_ipc::naming::pipe_user()?, Some(&t)),
    }
}

/// The tagged pipe of `app` when [`ENV`] is set, else `None` (the library's own default then
/// applies).
pub fn pipe_path_if_dev(app: &str) -> Option<String> {
    tag()?;
    pipe_path(app).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_normalised() {
        assert_eq!(parse_tag(None), None);
        assert_eq!(parse_tag(Some("")), None);
        assert_eq!(parse_tag(Some("  ")), None);
        assert_eq!(parse_tag(Some("--")), None);
        assert_eq!(parse_tag(Some("Desk1")).as_deref(), Some("desk1"));
        assert_eq!(parse_tag(Some(r"a b\c")).as_deref(), Some("a-b-c"));
        assert_eq!(parse_tag(Some(&"x".repeat(50))).unwrap().len(), MAX_TAG);
    }

    #[test]
    fn tagged_pipes_differ_from_the_real_ones() {
        let real = pipe_path_with("connector-ams2", "Alex", None).unwrap();
        let dev = pipe_path_with("connector-ams2", "Alex", Some("t1")).unwrap();
        assert_eq!(real, r"\\.\pipe\stintmate-connector-ams2-alex");
        assert_eq!(dev, r"\\.\pipe\stintmate-connector-ams2-alex-t1");
        let pw = pipe_path_with("pitwall", "Alex", Some("t1")).unwrap();
        assert_eq!(pw, r"\\.\pipe\stintmate-pitwall-alex-t1");
    }
}
