use super::*;
use clap::CommandFactory;
use clap::error::ErrorKind;

#[test]
fn cli_definition_is_valid() {
    Cli::command().debug_assert();
}

#[test]
fn only_the_service_logs_what_it_does() {
    let level = |args: &[&str]| log_level(Cli::try_parse_from(args).unwrap().command.as_ref());

    assert_eq!(level(&["dalsegno", "serve"]), LevelFilter::INFO);
    assert_eq!(level(&["dalsegno", "doctor"]), LevelFilter::WARN);
    assert_eq!(level(&["dalsegno"]), LevelFilter::WARN);
}

#[test]
fn version_flag_is_reported() {
    let err = Cli::try_parse_from(["dalsegno", "--version"]).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::DisplayVersion);
}

#[test]
fn fetch_takes_a_range() {
    let cli = Cli::try_parse_from([
        "dalsegno",
        "fetch",
        "abcDEF123_-",
        "--itag",
        "140",
        "--first",
        "1",
        "--last",
        "2",
    ])
    .unwrap();

    let Some(Command::Fetch { itag, last, .. }) = cli.command else {
        panic!("expected fetch");
    };
    assert_eq!((itag, last), (140, 2));
}

#[test]
fn backfill_hours_are_optional() {
    let hours = |args: &[&str]| match Cli::try_parse_from(args).unwrap().command {
        Some(Command::Backfill { hours, .. }) => hours,
        _ => panic!("expected backfill"),
    };

    assert_eq!(hours(&["dalsegno", "backfill", "abcDEF123_-"]), None);
    assert_eq!(
        hours(&["dalsegno", "backfill", "abcDEF123_-", "--hours", "6"]),
        Some(6.0)
    );
}

#[test]
fn set_takes_pairs_and_resets() {
    let cli = Cli::try_parse_from([
        "dalsegno",
        "set",
        "concurrency=2",
        "--reset",
        "daily_summary",
        "daily_summary=off",
    ])
    .unwrap();

    let Some(Command::Set { pairs, resets, .. }) = cli.command else {
        panic!("expected set");
    };
    assert_eq!(pairs, ["concurrency=2", "daily_summary=off"]);
    assert_eq!(resets, ["daily_summary"]);
}

#[test]
fn set_targets_a_stream_or_a_program_but_not_both_or_a_reset() {
    let parse = |args: &[&str]| Cli::try_parse_from(["dalsegno", "set"].iter().chain(args));

    assert!(parse(&["--stream", "abcDEF123_-", "title=Example"]).is_ok());
    assert!(parse(&["--program", "2", "artist=Example"]).is_ok());
    assert!(parse(&["--stream", "abcDEF123_-", "--program", "2"]).is_err());
    assert!(parse(&["--program", "2", "--reset", "concurrency"]).is_err());
}

#[test]
fn known_adds_only_with_a_whole_stretch() {
    let parse = |args: &[&str]| Cli::try_parse_from(["dalsegno", "known", "2"].iter().chain(args));

    assert!(parse(&[]).is_ok());
    assert!(parse(&["--remove", "5"]).is_ok());
    assert!(
        parse(&[
            "--add",
            "Opening",
            "--stream",
            "abcDEF123_-",
            "--from",
            "1",
            "--to",
            "9"
        ])
        .is_ok()
    );
    assert!(parse(&["--add", "Opening"]).is_err());
    assert!(parse(&["--stream", "abcDEF123_-"]).is_err());
    assert!(
        parse(&[
            "--add", "x", "--stream", "a", "--from", "1", "--to", "2", "--remove", "5"
        ])
        .is_err()
    );
}

#[test]
fn request_limits_come_from_the_arguments_or_the_defaults() {
    let given = Cli::try_parse_from([
        "dalsegno",
        "--requests-per-second",
        "2.5",
        "--concurrency",
        "1",
        "doctor",
    ])
    .unwrap();
    let defaults = Cli::try_parse_from(["dalsegno", "doctor"]).unwrap();

    assert_eq!((given.requests_per_second, given.concurrency), (2.5, 1));
    assert_eq!(
        (defaults.requests_per_second, defaults.concurrency),
        (4.0, 4)
    );
}

#[test]
fn unknown_ip_policy_is_rejected() {
    let error = Cli::try_parse_from(["dalsegno", "--ip-policy", "ipv4", "doctor"]).unwrap_err();

    assert_eq!(error.kind(), ErrorKind::ValueValidation);
}

#[test]
fn diff_is_offline_with_an_env_and_online_with_a_change() {
    let parse = |args: &[&str]| Cli::try_parse_from(args);

    let offline = parse(&[
        "dalsegno",
        "diff",
        "--env",
        "a.npy",
        "--period",
        "60",
        "--against",
        "b.npy",
        "--against-period",
        "50",
    ])
    .unwrap();
    let Some(Command::Diff { env, change, .. }) = offline.command else {
        panic!("expected diff");
    };
    assert_eq!((env.is_some(), change), (true, None));
    let Some(Command::Diff { env, change, .. }) =
        parse(&["dalsegno", "diff", "7"]).unwrap().command
    else {
        panic!("expected diff");
    };
    assert_eq!((env.is_some(), change), (false, Some(7)));
    assert!(parse(&["dalsegno", "diff"]).is_err());
    assert!(parse(&["dalsegno", "diff", "--env", "a.npy"]).is_err());
    assert!(parse(&["dalsegno", "diff", "7", "--env", "a.npy"]).is_err());
}

#[test]
fn clip_creates_from_a_change_and_edits_by_its_id() {
    let parse = |args: &[&str]| Cli::try_parse_from(args);

    let created = parse(&[
        "dalsegno", "clip", "--change", "3", "--kind", "addition", "--from", "1.5", "--to", "9",
    ])
    .unwrap();
    let Some(Command::Clip { clip, change, .. }) = created.command else {
        panic!("expected clip");
    };
    assert_eq!((clip, change), (None, Some(3)));
    let edited = parse(&["dalsegno", "clip", "5", "--to", "12"]).unwrap();
    let Some(Command::Clip { clip, to, .. }) = edited.command else {
        panic!("expected clip");
    };
    assert_eq!((clip, to), (Some(5), Some(12.0)));
    assert!(parse(&["dalsegno", "clip"]).is_err());
    assert!(parse(&["dalsegno", "clip", "--change", "3"]).is_err());
    assert!(parse(&["dalsegno", "clip", "5", "--change", "3"]).is_err());
}
