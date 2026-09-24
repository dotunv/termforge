use super::*;
use proptest::prelude::*;

fn events(input: &[u8]) -> Vec<Located> {
    let mut tap = Tap::new();
    let mut out = Vec::new();
    tap.feed(input, &mut out);
    out
}

fn kinds(input: &[u8]) -> Vec<TapEvent> {
    events(input).into_iter().map(|l| l.event).collect()
}

#[test]
fn osc133_marks_with_bel_and_st() {
    let input = b"\x1b]133;A\x07PS> \x1b]133;B\x1b\\ls\r\n\x1b]133;C\x07out\x1b]133;D;2\x07";
    assert_eq!(
        kinds(input),
        vec![
            TapEvent::Mark(Mark::PromptStart),
            TapEvent::Mark(Mark::InputStart),
            TapEvent::Mark(Mark::CommandExecuted),
            TapEvent::Mark(Mark::CommandFinished { exit_code: Some(2) }),
        ]
    );
}

#[test]
fn osc133_d_without_code_and_with_options() {
    assert_eq!(
        kinds(b"\x1b]133;D\x07\x1b]133;A;cl=m\x07"),
        vec![
            TapEvent::Mark(Mark::CommandFinished { exit_code: None }),
            TapEvent::Mark(Mark::PromptStart),
        ]
    );
}

#[test]
fn offset_points_just_past_terminator() {
    let input = b"ab\x1b]133;C\x07cd";
    let ev = events(input);
    assert_eq!(ev.len(), 1);
    assert_eq!(&input[ev[0].offset..], b"cd");
}

#[test]
fn cwd_from_osc7_windows_and_posix() {
    assert_eq!(
        kinds(b"\x1b]7;file://HOST/C:/Users/Dotun/My%20Code\x07"),
        vec![TapEvent::Cwd("C:/Users/Dotun/My Code".into())]
    );
    assert_eq!(
        kinds(b"\x1b]7;file://box/home/dotun\x1b\\"),
        vec![TapEvent::Cwd("/home/dotun".into())]
    );
}

#[test]
fn cwd_from_osc9_9_quoted_and_unquoted() {
    assert_eq!(
        kinds(b"\x1b]9;9;\"C:\\Users\\dotun\"\x07"),
        vec![TapEvent::Cwd("C:\\Users\\dotun".into())]
    );
    assert_eq!(
        kinds(b"\x1b]9;9;D:\\src\x07"),
        vec![TapEvent::Cwd("D:\\src".into())]
    );
}

#[test]
fn notifications() {
    assert_eq!(
        kinds(b"\x1b]777;notify;Claude;Needs input\x07"),
        vec![TapEvent::Notify {
            title: Some("Claude".into()),
            body: "Needs input".into()
        }]
    );
    assert_eq!(
        kinds(b"\x1b]9;Build done\x07"),
        vec![TapEvent::Notify {
            title: None,
            body: "Build done".into()
        }]
    );
    assert_eq!(
        kinds(b"\x1b]99;;Tests passed\x1b\\"),
        vec![TapEvent::Notify {
            title: None,
            body: "Tests passed".into()
        }]
    );
    assert_eq!(kinds(b"\x1b]99;d=0;partial\x07"), vec![]);
}

#[test]
fn progress() {
    assert_eq!(
        kinds(b"\x1b]9;4;1;42\x07\x1b]9;4;0\x07"),
        vec![
            TapEvent::Progress {
                state: ProgressState::Normal,
                percent: Some(42)
            },
            TapEvent::Progress {
                state: ProgressState::Clear,
                percent: None
            },
        ]
    );
}

#[test]
fn ignores_unknown_and_other_escapes() {
    assert_eq!(
        kinds(b"\x1b[31mred\x1b[0m\x1b]0;title\x07\x1b]1337;SetMark\x07"),
        vec![]
    );
    assert_eq!(kinds(b"\x1b]9;1;500\x07"), vec![]);
}

#[test]
fn can_aborts_sequence() {
    assert_eq!(
        kinds(b"\x1b]133;A\x18\x1b]133;B\x07"),
        vec![TapEvent::Mark(Mark::InputStart)]
    );
}

#[test]
fn esc_without_backslash_restarts() {
    // The first OSC is cancelled by ESC ']' which starts a new OSC.
    assert_eq!(
        kinds(b"\x1b]133;A\x1b]133;C\x07"),
        vec![TapEvent::Mark(Mark::CommandExecuted)]
    );
}

#[test]
fn overlong_payload_is_discarded_and_parser_recovers() {
    let mut input = b"\x1b]1337;File=".to_vec();
    input.extend(std::iter::repeat_n(b'A', MAX_OSC_LEN * 3));
    input.extend_from_slice(b"\x07\x1b]133;A\x07");
    assert_eq!(kinds(&input), vec![TapEvent::Mark(Mark::PromptStart)]);
}

#[test]
fn invalid_utf8_payload_is_ignored() {
    assert_eq!(kinds(b"\x1b]9;\xff\xfe\x07"), vec![]);
}

proptest! {
    /// Splitting the stream at any point must not change what is detected.
    #[test]
    fn chunking_is_transparent(split in 0usize..80) {
        let input: &[u8] = b"x\x1b]133;A\x07PS> \x1b]133;B\x1b\\dir\r\n\x1b]133;C\x07ok\x1b]7;file://h/C:/a%20b\x07\x1b]133;D;0\x07";
        let split = split.min(input.len());
        let whole = kinds(input);
        let mut tap = Tap::new();
        let mut out = Vec::new();
        tap.feed(&input[..split], &mut out);
        tap.feed(&input[split..], &mut out);
        let chunked: Vec<_> = out.into_iter().map(|l| l.event).collect();
        prop_assert_eq!(whole, chunked);
    }

    /// The tap never panics and never buffers more than the limit.
    #[test]
    fn arbitrary_bytes_are_safe(bytes in proptest::collection::vec(any::<u8>(), 0..8192)) {
        let mut tap = Tap::new();
        let mut out = Vec::new();
        tap.feed(&bytes, &mut out);
        prop_assert!(tap.buf.len() <= MAX_OSC_LEN);
        for l in &out { prop_assert!(l.offset <= bytes.len()); }
    }
}
