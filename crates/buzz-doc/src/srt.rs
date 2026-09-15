//! **SubRip subtitles** — reading and writing `.srt`.
//!
//! # Why a subtitle file is worth reading into an animation tool
//!
//! Because it is the only place the *words* of a narration and their *timings*
//! sit in one file. The program can already hear where a voice-over speaks
//! (`buzz_audio::detect_phrases`) but not what it says, and knowing what is
//! said and when is the missing half of several things at once: captions burnt
//! onto the picture, a keyframe per line to draw against, and — the one that
//! matters most — dialogue that can be handed to a *named character*, which is
//! what lip sync and the director have both been missing.
//!
//! Nobody has to type any of it. Any transcription tool writes this format,
//! and so does YouTube.
//!
//! # The format
//!
//! ```text
//! 1
//! 00:00:01,000 --> 00:00:04,200
//! Ana: We should go before it gets dark.
//!
//! 2
//! 00:00:05,100 --> 00:00:07,800
//! Ben said nothing.
//! ```
//!
//! A number, a timecode pair, one or more lines of text, a blank line. That is
//! the whole specification, which is why every tool on earth emits it.
//!
//! # Where this is lenient, and where it is not
//!
//! Real subtitle files are written by a hundred different programs and a fair
//! number of them are careless. So:
//!
//! * A **byte-order mark** is stripped. Windows tools add one constantly.
//! * The **index line is optional**, and any index is accepted; the cues are
//!   ordered by their timecodes rather than by what they claim to be numbered.
//! * **Milliseconds may be separated by a comma or a full stop.** SRT says
//!   comma, WebVTT says stop, and half the tools in the world emit the wrong
//!   one.
//! * **Markup is stripped** — `<i>`, `<font …>`, `{\an8}`. The text becomes
//!   *artwork* here, and a literal `<i>` drawn on the picture is worse than an
//!   unitalicised line.
//! * A **block that cannot be read is skipped and counted**, not fatal. One
//!   mangled cue in a thousand should not cost the other nine hundred and
//!   ninety-nine.
//!
//! What is *not* forgiven is a file with no readable cue in it at all: that is
//! reported, because it means the wrong file was chosen and silence would send
//! the user looking in the wrong place.

use std::fmt::Write as _;

/// One subtitle: what is said, and between which two moments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    /// Milliseconds from the start of the film.
    pub start_ms: u64,
    /// Milliseconds from the start of the film. Never before `start_ms`.
    pub end_ms: u64,
    /// The line, with any markup removed. May hold newlines: a subtitle broken
    /// over two lines is broken deliberately and the break is kept.
    pub text: String,
    /// **Who says it**, when the line names somebody: `Ana: We should go.`
    ///
    /// Split out rather than left in the text because it is the hook everything
    /// interesting hangs off — routing a line to a character's mouth, or
    /// handing the director a cast. [`Self::text`] keeps the words without the
    /// name, so a caption drawn on the picture does not read "Ana: Ana: …".
    pub speaker: Option<String>,
}

impl Cue {
    /// The frames this cue covers, at `fps`, as a half-open range.
    ///
    /// Rounded rather than truncated at both ends: a cue that starts 4 ms into
    /// a frame started on that frame as far as anybody watching is concerned,
    /// and truncating would show every caption one frame late.
    pub fn frames(&self, fps: f64) -> std::ops::Range<u32> {
        let to_frame = |ms: u64| ((ms as f64 / 1000.0) * fps).round().max(0.0) as u32;
        let start = to_frame(self.start_ms);
        // At least one frame long: a zero-length caption is one nobody can see
        // and one the timeline cannot hold a keyframe for.
        let end = to_frame(self.end_ms).max(start + 1);
        start..end
    }

    /// How long the cue lasts, in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// What a read produced, and what it could not read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Captions {
    /// The cues, in time order.
    pub cues: Vec<Cue>,
    /// Blocks that could not be read. Counted rather than dropped silently, so
    /// a file that is half-corrupt says so.
    pub skipped: usize,
}

impl Captions {
    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }

    /// Everybody named in the file, in the order they first speak.
    ///
    /// The cast of the narration, which is what a director would want handed to
    /// it — see the module note.
    pub fn speakers(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for cue in &self.cues {
            if let Some(who) = &cue.speaker
                && !out.iter().any(|s| s.eq_ignore_ascii_case(who))
            {
                out.push(who.clone());
            }
        }
        out
    }
}

/// **Read a subtitle file.**
///
/// Never fails: an unreadable block is counted in [`Captions::skipped`] and the
/// rest is returned. An empty result means nothing in the text looked like a
/// cue, which the caller should report rather than treat as "no subtitles".
pub fn parse(text: &str) -> Captions {
    // A byte-order mark on the front would make the first index line
    // unparseable and cost the first cue of every file a Windows tool wrote.
    let text = text.trim_start_matches('\u{feff}');

    let mut cues: Vec<Cue> = Vec::new();
    let mut skipped = 0usize;

    // Blocks are separated by blank lines. Carriage returns are stripped per
    // line rather than up front, so a file with mixed endings still splits.
    let mut block: Vec<&str> = Vec::new();
    let flush = |block: &mut Vec<&str>, cues: &mut Vec<Cue>, skipped: &mut usize| {
        if block.is_empty() {
            return;
        }
        match read_block(block) {
            Some(cue) => cues.push(cue),
            None => *skipped += 1,
        }
        block.clear();
    };

    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            flush(&mut block, &mut cues, &mut skipped);
        } else {
            block.push(line);
        }
    }
    flush(&mut block, &mut cues, &mut skipped);

    // **Ordered by time, not by the numbers in the file.** The index line is
    // decoration; some tools renumber badly and some do not number at all.
    cues.sort_by_key(|c| (c.start_ms, c.end_ms));
    name_the_speakers(&mut cues);
    Captions { cues, skipped }
}

/// **Decide which `Name:` prefixes are really speakers**, across the whole file.
///
/// # Why this cannot be decided one line at a time
///
/// `Ana: We should go.` names a speaker. `Meanwhile: the door opened.` does
/// not, and no amount of looking at that one line will tell them apart — both
/// are a capitalised word, a colon and a sentence.
///
/// What separates them is the **file**. A character speaks more than once; a
/// sentence adverb is a turn of phrase that happens to be at the front of one
/// line. So a prefix is taken as a speaker when it either
///
/// * appears on **two or more cues** — somebody with a part, or
/// * is written in **capitals** (`ANA:`), which is the broadcast convention and
///   is never how prose begins a sentence.
///
/// This is a stop-list-free rule, which matters: a list of words like
/// "Meanwhile" and "However" would be a list of *English* words, and the
/// program would then quietly cast a character in every other language.
///
/// # Erring towards keeping the words
///
/// A prefix that is not confirmed is **left in the text**. Getting it wrong in
/// that direction costs a caption that reads "Meanwhile: the door opened",
/// which is what the writer typed; getting it wrong the other way silently
/// deletes a word from the picture and invents a character called Meanwhile.
fn name_the_speakers(cues: &mut [Cue]) {
    use std::collections::HashMap;

    let mut seen: HashMap<String, usize> = HashMap::new();
    for cue in cues.iter() {
        if let Some((name, _)) = name_prefix(&cue.text) {
            *seen.entry(name.to_lowercase()).or_default() += 1;
        }
    }

    for cue in cues.iter_mut() {
        let Some((name, rest)) = name_prefix(&cue.text) else {
            continue;
        };
        let recurs = seen.get(&name.to_lowercase()).copied().unwrap_or(0) >= 2;
        let shouted = name.chars().any(|c| c.is_alphabetic())
            && name.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase());
        if recurs || shouted {
            cue.speaker = Some(name.to_string());
            cue.text = rest.to_string();
        }
    }
}

/// One block: an optional index, a timecode line, then the words.
fn read_block(lines: &[&str]) -> Option<Cue> {
    // The timecode is the line with the arrow in it. Found rather than assumed
    // to be second, which is what makes the index line optional.
    let at = lines.iter().position(|l| l.contains("-->"))?;
    let (start_ms, end_ms) = read_timecodes(lines[at])?;

    let body = lines[at + 1..].join("\n");
    let text = strip_markup(&body).trim().to_string();
    if text.is_empty() {
        return None;
    }

    Some(Cue {
        start_ms,
        // A cue that ends before it starts is a broken cue; holding the end at
        // the start keeps every consumer's arithmetic non-negative.
        end_ms: end_ms.max(start_ms),
        text,
        // Decided over the whole file once every cue is read — see `name_the_speakers`.
        speaker: None,
    })
}

/// `00:00:01,000 --> 00:00:04,200`, in milliseconds.
fn read_timecodes(line: &str) -> Option<(u64, u64)> {
    let (a, b) = line.split_once("-->")?;
    Some((read_timecode(a)?, read_timecode(b)?))
}

/// `HH:MM:SS,mmm`. Hours may be missing, and the millisecond separator may be
/// a comma or a stop — see the module note on leniency.
fn read_timecode(field: &str) -> Option<u64> {
    // Trailing cue settings (`line:0%`, `align:start`) are WebVTT's, and turn
    // up in files renamed to .srt. Everything past the first space is dropped.
    let field = field.trim().split_whitespace().next()?;
    let (clock, millis) = match field.rsplit_once([',', '.']) {
        Some((clock, ms)) => (clock, ms.parse::<u64>().ok()?),
        None => (field, 0),
    };

    let mut seconds = 0u64;
    let mut parts = 0;
    for part in clock.split(':') {
        seconds = seconds.checked_mul(60)?.checked_add(part.trim().parse::<u64>().ok()?)?;
        parts += 1;
    }
    if parts == 0 || parts > 3 {
        return None;
    }
    // Three digits is milliseconds; two would be hundredths. Scaled by what was
    // actually written rather than assumed, because both turn up.
    let millis = match millis {
        m if m < 10 => m * 100,
        m if m < 100 => m * 10,
        m => m,
    };
    Some(seconds * 1000 + millis)
}

/// Remove `<i>`, `<font color="#fff">` and `{\an8}`.
///
/// The text here becomes **artwork**, and a literal `<i>` drawn onto the
/// picture is worse than a line that is not italic.
fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth_angle = 0usize;
    let mut depth_brace = 0usize;
    for c in text.chars() {
        match c {
            '<' => depth_angle += 1,
            '>' => depth_angle = depth_angle.saturating_sub(1),
            '{' => depth_brace += 1,
            '}' => depth_brace = depth_brace.saturating_sub(1),
            _ if depth_angle == 0 && depth_brace == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// `Ana: We should go.` into the candidate name and the words after it.
///
/// **Shape only, no verdict**: whether this candidate is really a speaker is a
/// question about the whole file, and [`name_the_speakers`] answers it. What is
/// checked here is that the head could be a name at all — one or two words,
/// capitalised, letters, and short. A URL, a time and a sentence with a colon
/// in the middle of it all fail on those.
fn name_prefix(text: &str) -> Option<(&str, &str)> {
    let (head, rest) = text.split_once(':')?;
    let head = head.trim();
    let rest = rest.trim_start();
    if head.is_empty() || rest.is_empty() {
        return None;
    }
    // A name is short, capitalised and made of letters. Two words at most: a
    // first name and a surname, or "Old Man".
    let words: Vec<&str> = head.split_whitespace().collect();
    let name_shaped = (1..=2).contains(&words.len())
        && head.chars().count() <= 24
        && words.iter().all(|w| {
            w.chars().next().is_some_and(|c| c.is_uppercase())
                && w.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '-')
        });
    name_shaped.then_some((head, rest))
}

/// **Write cues out as `.srt`.**
///
/// Numbered from one, in time order, with a comma before the milliseconds —
/// the letter of the format, whatever this reader is willing to accept.
pub fn write(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (i, cue) in cues.iter().enumerate() {
        let _ = writeln!(out, "{}", i + 1);
        let _ = writeln!(
            out,
            "{} --> {}",
            timecode(cue.start_ms),
            timecode(cue.end_ms.max(cue.start_ms))
        );
        // The speaker goes back on the front, so a file read in and written out
        // again is the same file.
        match &cue.speaker {
            Some(who) => {
                let _ = writeln!(out, "{who}: {}", cue.text);
            }
            None => {
                let _ = writeln!(out, "{}", cue.text);
            }
        }
        out.push('\n');
    }
    out
}

fn timecode(ms: u64) -> String {
    let (h, rest) = (ms / 3_600_000, ms % 3_600_000);
    let (m, rest) = (rest / 60_000, rest % 60_000);
    let (s, milli) = (rest / 1000, rest % 1000);
    format!("{h:02}:{m:02}:{s:02},{milli:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
1
00:00:01,000 --> 00:00:04,200
Ana: We should go before it gets dark.

2
00:00:05,100 --> 00:00:07,800
Ben said nothing.
";

    #[test]
    fn an_ordinary_file_reads() {
        let caps = parse(SAMPLE);
        assert_eq!(caps.skipped, 0);
        assert_eq!(caps.cues.len(), 2);
        assert_eq!(caps.cues[0].start_ms, 1_000);
        assert_eq!(caps.cues[0].end_ms, 4_200);
        // Ana speaks once in this sample, so her name stays in the line — see
        // `a_word_that_appears_once_is_not_a_speaker` for why that is the safe
        // direction to be wrong in.
        assert_eq!(caps.cues[0].text, "Ana: We should go before it gets dark.");
        assert_eq!(caps.cues[0].speaker, None);
        assert_eq!(caps.cues[1].speaker, None);
    }

    /// **A file written out and read back is the same file.** The property that
    /// makes it safe to round-trip captions through the program.
    #[test]
    fn writing_and_reading_come_back_to_the_same_cues() {
        let caps = parse(SAMPLE);
        let again = parse(&write(&caps.cues));
        assert_eq!(caps.cues, again.cues);
    }

    /// **Timecodes go out in the letter of the format** — a comma, and two
    /// digits everywhere they belong.
    #[test]
    fn timecodes_are_written_the_way_the_format_says() {
        assert_eq!(timecode(0), "00:00:00,000");
        assert_eq!(timecode(1_000), "00:00:01,000");
        assert_eq!(timecode(3_661_042), "01:01:01,042");
    }

    /// **A byte-order mark does not cost the first cue.** Windows tools add one
    /// constantly and it lands exactly on the first index line.
    #[test]
    fn a_byte_order_mark_is_ignored() {
        let with_bom = format!("\u{feff}{SAMPLE}");
        assert_eq!(parse(&with_bom).cues.len(), 2);
    }

    /// **A full stop before the milliseconds is accepted.** The format says
    /// comma; WebVTT says stop; half the tools in the world emit the wrong one.
    #[test]
    fn a_webvtt_style_separator_is_accepted() {
        let caps = parse("1\n00:00:02.500 --> 00:00:04.000\nHello.\n");
        assert_eq!(caps.cues.len(), 1);
        assert_eq!(caps.cues[0].start_ms, 2_500);
    }

    /// **The index line is optional**, because plenty of files do not have one.
    #[test]
    fn a_file_with_no_numbers_still_reads() {
        let caps = parse("00:00:01,000 --> 00:00:02,000\nOne.\n\n00:00:03,000 --> 00:00:04,000\nTwo.\n");
        assert_eq!(caps.cues.len(), 2, "{caps:?}");
    }

    /// **Cues come back in time order**, whatever the file numbered them.
    #[test]
    fn cues_are_sorted_by_time_not_by_their_numbers() {
        let caps = parse(
            "7\n00:00:09,000 --> 00:00:10,000\nLater.\n\n2\n00:00:01,000 --> 00:00:02,000\nEarlier.\n",
        );
        assert_eq!(caps.cues[0].text, "Earlier.");
        assert_eq!(caps.cues[1].text, "Later.");
    }

    /// **Markup is stripped**, because the text becomes artwork and a literal
    /// `<i>` drawn on the picture is worse than a line that is not italic.
    #[test]
    fn markup_does_not_end_up_on_the_screen() {
        let caps = parse(
            "1\n00:00:01,000 --> 00:00:02,000\n{\\an8}<i>She <font color=\"#fff\">ran</font>.</i>\n",
        );
        assert_eq!(caps.cues[0].text, "She ran.");
    }

    /// **A subtitle broken over two lines stays broken.** The break was a
    /// choice about how it reads on screen.
    #[test]
    fn a_two_line_subtitle_keeps_its_break() {
        let caps = parse("1\n00:00:01,000 --> 00:00:03,000\nShe opened the door\nand went out.\n");
        assert_eq!(caps.cues[0].text, "She opened the door\nand went out.");
    }

    /// **One bad block does not cost the file.** It is counted so the reader
    /// can say how many, rather than dropped in silence.
    #[test]
    fn a_broken_block_is_skipped_and_counted() {
        let caps = parse(
            "1\n00:00:01,000 --> 00:00:02,000\nGood.\n\nnonsense\nwith no timecode\n\n3\n00:00:05,000 --> 00:00:06,000\nAlso good.\n",
        );
        assert_eq!(caps.cues.len(), 2);
        assert_eq!(caps.skipped, 1);
    }

    /// **Nothing readable comes back as nothing**, so the caller can say the
    /// file was wrong instead of showing an empty timeline.
    #[test]
    fn a_file_that_is_not_subtitles_reads_as_empty() {
        assert!(parse("").is_empty());
        assert!(parse("This is just a paragraph of prose.\n").is_empty());
    }

    /// **Nothing that is merely name-shaped is a speaker.** A URL, a time, and
    /// a sentence with a colon in it all have to fail.
    #[test]
    fn only_a_name_shape_can_be_a_speaker_at_all() {
        for line in [
            "See https://example.com for more.",
            "No colon here at all.",
            "At 3: the door opened.",
            "A very long stretch of narration indeed: and then more.",
        ] {
            let text = format!("1\n00:00:01,000 --> 00:00:02,000\n{line}\n");
            let cue = parse(&text).cues.remove(0);
            assert_eq!(cue.speaker, None, "{line:?} was read as dialogue");
            assert_eq!(cue.text, line, "{line:?} lost words from the caption");
        }
    }

    /// **A one-off `Word:` is prose, not a character.**
    ///
    /// "Meanwhile: the door opened" is exactly as name-shaped as "Ana: hello",
    /// and the only thing that tells them apart is that a character comes back.
    /// Getting this wrong deletes a word from the picture and casts somebody
    /// called Meanwhile.
    #[test]
    fn a_word_that_appears_once_is_not_a_speaker() {
        let caps = parse("1\n00:00:01,000 --> 00:00:02,000\nMeanwhile: the door opened.\n");
        assert_eq!(caps.cues[0].speaker, None);
        assert_eq!(caps.cues[0].text, "Meanwhile: the door opened.", "words were lost");
    }

    /// **Somebody who speaks twice is a character.** The whole-file rule.
    #[test]
    fn a_name_that_comes_back_is_a_speaker() {
        let caps = parse(
            "1\n00:00:01,000 --> 00:00:02,000\nAna: Hello.\n\n\
             2\n00:00:03,000 --> 00:00:04,000\nAna: Still here.\n",
        );
        assert_eq!(caps.cues[0].speaker.as_deref(), Some("Ana"));
        assert_eq!(caps.cues[0].text, "Hello.");
        assert_eq!(caps.speakers(), vec!["Ana".to_string()]);
    }

    /// **Capitals are the broadcast convention**, and prose never starts a
    /// sentence that way — so one line of it is enough.
    #[test]
    fn a_shouted_name_is_a_speaker_first_time() {
        let caps = parse("1\n00:00:01,000 --> 00:00:02,000\nANA: Hello.\n");
        assert_eq!(caps.cues[0].speaker.as_deref(), Some("ANA"));
        assert_eq!(caps.cues[0].text, "Hello.");
    }

    /// **Two words can be a name**, because plenty are.
    #[test]
    fn a_two_word_name_works() {
        let caps = parse(
            "1\n00:00:01,000 --> 00:00:02,000\nOld Man: Hello.\n\n\
             2\n00:00:03,000 --> 00:00:04,000\nOld Man: Again.\n",
        );
        assert_eq!(caps.cues[0].speaker.as_deref(), Some("Old Man"));
    }

    /// **The cast is who speaks, in the order they first do.**
    #[test]
    fn the_speakers_are_the_cast_in_order() {
        let caps = parse(
            "1\n00:00:01,000 --> 00:00:02,000\nBen: First.\n\n\
             2\n00:00:03,000 --> 00:00:04,000\nAna: Second.\n\n\
             3\n00:00:05,000 --> 00:00:06,000\nBen: Again.\n\n\
             4\n00:00:07,000 --> 00:00:08,000\nAna: And again.\n",
        );
        assert_eq!(caps.speakers(), vec!["Ben".to_string(), "Ana".to_string()]);
    }

    /// **Milliseconds are rounded to frames, not truncated.** Truncating shows
    /// every caption a frame late, which is visible on a hard cut.
    #[test]
    fn cues_land_on_the_nearest_frame() {
        let cue = Cue {
            start_ms: 1_000,
            end_ms: 2_000,
            text: "x".into(),
            speaker: None,
        };
        assert_eq!(cue.frames(24.0), 24..48);

        // 41.7 ms is one frame at 24fps; 62 ms is nearer to frame 1 than 2.
        let cue = Cue {
            start_ms: 62,
            end_ms: 100,
            text: "x".into(),
            speaker: None,
        };
        let range = cue.frames(24.0);
        assert_eq!(range.start, 1);
        assert!(range.end > range.start, "a cue must last at least a frame");
    }

    /// **A cue that ends before it starts does not produce a backwards range**,
    /// which every consumer downstream would divide by.
    #[test]
    fn a_backwards_cue_is_held_rather_than_inverted() {
        let caps = parse("1\n00:00:05,000 --> 00:00:01,000\nBroken.\n");
        let cue = &caps.cues[0];
        assert!(cue.end_ms >= cue.start_ms);
        let range = cue.frames(24.0);
        assert!(range.end > range.start);
    }
}
