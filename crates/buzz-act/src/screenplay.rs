//! **A screenplay in, scenes out**: the markdown script form, read into
//! sluglines, cues, dialogue, action and pauses.
//!
//! # The form it reads
//!
//! The series scripts are plain markdown, written to be read by a person first:
//!
//! ```text
//! ## SCENE 4 — THE THREE BEROZGAARS
//!
//! **EXT. CHHOTU'S TEA STALL — MORNING**
//!
//! Tin roof.
//!
//! **NARRATOR (V.O.)**
//! Guddu.
//!
//! **GUDDU**
//! Main soch raha hoon kuch bada karun.
//!
//! Beat.
//!
//! ---
//! ```
//!
//! * `## SCENE n — TITLE` starts a scene.
//! * `**EXT. … — TIME**` / `**INT. … — TIME**` is the slugline: where, and when.
//! * A line that is only `**NAME**`, `**NAME (V.O.)**` or `**NAME (O.S.)**` is a
//!   **cue**; the non-blank lines after it, up to the next blank line, are what
//!   that character says.
//! * `---` is a **beat break**. `Beat.`, `Another beat.`, `Long silence.`,
//!   `Silence.` and `A pause.` are **pauses**.
//! * Everything else is **action**.
//! * `# END` ends the script; whatever follows it is the writer's notes.
//!
//! # What it does not do
//!
//! It does not know who the characters are. That is the casting file's, and a
//! cue is just a name here. It also does not time anything: see
//! [`crate::script_timing`], which reads the parsed scene and says how long
//! each part of it takes.
//!
//! # Bold lines that are not cues
//!
//! A sign in the script (`**BIKAU HAI**`, `**CHIRAILIYA — 2 KM**`) or a
//! transition (`**CUT TO BLACK.**`) is bold and often upper-case too. What
//! tells a cue from them is that a cue has something to say: a bold line with
//! no dialogue under it is action, kept with its words so a caption or a prop
//! can use them.

/// A whole script: a title and its scenes, in order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Screenplay {
    /// The `# ` heading before the first scene, without its marker.
    pub title: String,
    pub scenes: Vec<ScriptScene>,
}

impl Screenplay {
    /// Read a script. Never fails: a line that means nothing is action, and a
    /// file with no scenes is an empty screenplay, which the caller can see.
    pub fn parse(source: &str) -> Self {
        parse(source)
    }

    /// The scene numbered `n` in the script (not the n-th in the file: the
    /// number the writer gave it).
    pub fn scene(&self, number: u32) -> Option<&ScriptScene> {
        self.scenes.iter().find(|s| s.number == number)
    }
}

/// One scene of the script.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptScene {
    /// The writer's number: `## SCENE 4 — …` is 4.
    pub number: u32,
    /// What follows the dash in the heading, as written.
    pub title: String,
    /// The first slugline, if the scene has one. Scene 5 carries on from
    /// scene 4's place and has none.
    pub slugline: Option<Slugline>,
    /// Everything that happens, in order.
    pub items: Vec<Item>,
}

/// Interior or exterior, from the slugline's first word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Exterior,
    Interior,
    Other,
}

/// `EXT. CHHOTU'S TEA STALL — MORNING`, taken apart.
#[derive(Debug, Clone, PartialEq)]
pub struct Slugline {
    /// As written, without the asterisks.
    pub raw: String,
    pub place: Place,
    /// `CHHOTU'S TEA STALL`.
    pub location: String,
    /// `MORNING`; empty when the slugline gives none.
    pub time: String,
}

/// How a line is heard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voice {
    /// The speaker is in the shot.
    OnScreen,
    /// `(V.O.)` — a voice over the picture; nobody on screen speaks.
    VoiceOver,
    /// `(O.S.)` — the speaker is in the scene but out of the shot.
    OffScreen,
}

/// What a pause is called in the script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pause {
    /// `Beat.` / `Another beat.`
    Beat,
    /// `Long silence.`
    LongSilence,
    /// `Silence.` / `A pause.`
    Silence,
}

/// One thing a character says.
#[derive(Debug, Clone, PartialEq)]
pub struct Dialogue {
    /// The cue's name, as written and without the parenthetical: `NARRATOR`,
    /// `SHARMA JI`.
    pub speaker: String,
    pub voice: Voice,
    /// The words, the cue's lines joined with a space.
    pub text: String,
}

/// One beat of a scene.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A paragraph of stage direction.
    Action(String),
    Dialogue(Dialogue),
    Pause(Pause),
    /// `---`.
    Break,
    /// A second slugline inside a scene: the place changes mid-scene.
    Slugline(Slugline),
}

// -- the parser ------------------------------------------------------------------

fn parse(source: &str) -> Screenplay {
    let text = source.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let lines: Vec<&str> = text.lines().collect();

    let mut out = Screenplay::default();
    let mut current: Option<ScriptScene> = None;
    let mut i = 0usize;

    while i < lines.len() {
        let raw = lines[i];
        let line = raw.trim();

        // The notes after the script are not the script.
        if line.eq_ignore_ascii_case("# END") {
            break;
        }

        if line.is_empty() {
            i += 1;
            continue;
        }

        // A top-level heading before the first scene is the title.
        if let Some(rest) = line.strip_prefix("# ")
            && !line.starts_with("## ")
        {
            if out.title.is_empty() && current.is_none() {
                out.title = strip_markup(rest);
            }
            i += 1;
            continue;
        }

        if let Some(rest) = strip_scene_heading(line) {
            if let Some(done) = current.take() {
                out.scenes.push(done);
            }
            let (number, title) = split_scene_heading(rest, out.scenes.len() as u32 + 1);
            current = Some(ScriptScene {
                number,
                title,
                slugline: None,
                items: Vec::new(),
            });
            i += 1;
            continue;
        }

        // Anything before the first scene is the front matter.
        let Some(scene) = current.as_mut() else {
            i += 1;
            continue;
        };

        // `---`: a beat break.
        if is_rule(line) {
            scene.items.push(Item::Break);
            i += 1;
            continue;
        }

        // `### SHANTI COLONY` and friends: a sign card, so action.
        if line.starts_with('#') {
            let words = strip_markup(line.trim_start_matches('#'));
            if !words.is_empty() {
                scene.items.push(Item::Action(words));
            }
            i += 1;
            continue;
        }

        if let Some(inner) = bold_only(line) {
            if let Some(slug) = parse_slugline(inner) {
                // The first slugline names the scene's place; any later one is
                // a cut to somewhere else inside it.
                if scene.slugline.is_none() {
                    scene.slugline = Some(slug);
                } else {
                    scene.items.push(Item::Slugline(slug));
                }
                i += 1;
                continue;
            }

            if let Some((speaker, voice)) = parse_cue(inner) {
                let mut spoken: Vec<String> = Vec::new();
                let mut j = i + 1;
                while j < lines.len() {
                    let next = lines[j].trim();
                    if next.is_empty() || next.starts_with("**") || next.starts_with('#') || is_rule(next)
                    {
                        break;
                    }
                    spoken.push(strip_markup(next));
                    j += 1;
                }
                if !spoken.is_empty() {
                    scene.items.push(Item::Dialogue(Dialogue {
                        speaker,
                        voice,
                        text: spoken.join(" "),
                    }));
                    i = j;
                    continue;
                }
            }

            // A bold line with nothing under it: a sign or a transition.
            let words = strip_markup(inner);
            if !words.is_empty() {
                scene.items.push(Item::Action(words));
            }
            i += 1;
            continue;
        }

        if let Some(pause) = parse_pause(line) {
            scene.items.push(Item::Pause(pause));
            i += 1;
            continue;
        }

        // Action: the paragraph, up to the next blank or special line.
        let mut paragraph: Vec<String> = vec![strip_markup(line)];
        let mut j = i + 1;
        while j < lines.len() {
            let next = lines[j].trim();
            if next.is_empty()
                || next.starts_with('#')
                || is_rule(next)
                || bold_only(next).is_some()
                || parse_pause(next).is_some()
            {
                break;
            }
            paragraph.push(strip_markup(next));
            j += 1;
        }
        scene.items.push(Item::Action(paragraph.join(" ")));
        i = j;
    }

    if let Some(done) = current.take() {
        out.scenes.push(done);
    }
    out
}

/// The text after `## SCENE`, if this line is a scene heading.
fn strip_scene_heading(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("##")?;
    if rest.starts_with('#') {
        return None;
    }
    let rest = rest.trim_start();
    let head = rest.get(..5)?;
    if head.eq_ignore_ascii_case("scene") {
        Some(rest[5..].trim())
    } else {
        None
    }
}

/// `4 — THE THREE BEROZGAARS` into `(4, "THE THREE BEROZGAARS")`.
fn split_scene_heading(rest: &str, fallback: u32) -> (u32, String) {
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let number = digits.parse::<u32>().unwrap_or(fallback);
    let title = rest[digits.len()..]
        .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '—' | '–' | '-' | ':' | '.'))
        .trim();
    (number, strip_markup(title))
}

fn is_rule(line: &str) -> bool {
    line.len() >= 3 && line.chars().all(|c| c == '-')
}

/// `**WORDS**` with nothing else on the line: the words.
fn bold_only(line: &str) -> Option<&str> {
    let inner = line.strip_prefix("**")?.strip_suffix("**")?;
    if inner.is_empty() || inner.contains("**") {
        return None;
    }
    Some(inner.trim())
}

fn parse_slugline(inner: &str) -> Option<Slugline> {
    let upper = inner.to_uppercase();
    let (place, rest) = if let Some(r) = upper.strip_prefix("EXT.") {
        (Place::Exterior, r)
    } else if let Some(r) = upper.strip_prefix("INT.") {
        (Place::Interior, r)
    } else if upper.starts_with("INT/EXT") || upper.starts_with("I/E") {
        let at = upper.find('.').map(|p| p + 1).unwrap_or(0);
        (Place::Other, &upper[at..])
    } else {
        return None;
    };
    let rest = rest.trim();
    // Location and time are split at the last dash with spaces round it.
    let split = [" — ", " – ", " - "]
        .iter()
        .filter_map(|d| rest.rfind(d).map(|p| (p, d.len())))
        .max_by_key(|(p, _)| *p);
    let (location, time) = match split {
        Some((at, width)) => (rest[..at].trim(), rest[at + width..].trim()),
        None => (rest, ""),
    };
    Some(Slugline {
        raw: inner.trim().to_string(),
        place,
        location: location.to_string(),
        time: time.to_string(),
    })
}

/// `NARRATOR (V.O.)` into the name and how it is heard. `None` for a bold line
/// that cannot be somebody's name.
fn parse_cue(inner: &str) -> Option<(String, Voice)> {
    let inner = inner.trim();
    let (name, paren) = match inner.rfind('(') {
        Some(open) if inner.ends_with(')') => (
            inner[..open].trim(),
            inner[open + 1..inner.len() - 1].trim().to_uppercase(),
        ),
        _ => (inner, String::new()),
    };
    if !is_cue_name(name) {
        return None;
    }
    let voice = if paren.contains("V.O") || paren == "VO" {
        Voice::VoiceOver
    } else if paren.contains("O.S") || paren.contains("O.C") {
        Voice::OffScreen
    } else {
        Voice::OnScreen
    };
    Some((name.to_string(), voice))
}

/// Upper-case words, digits and the few marks a name carries: `SHARMA JI`,
/// `MAN 1`, `CHHOTU'S`.
fn is_cue_name(name: &str) -> bool {
    if name.is_empty() || name.chars().count() > 40 {
        return false;
    }
    let mut letters = 0;
    for c in name.chars() {
        if c.is_alphabetic() {
            if c.is_lowercase() {
                return false;
            }
            letters += 1;
        } else if !(c.is_ascii_digit() || matches!(c, ' ' | '.' | '\'' | '\u{2019}' | '-' | '&' | '/')) {
            return false;
        }
    }
    letters > 0
}

fn parse_pause(line: &str) -> Option<Pause> {
    let said = line.trim().trim_end_matches('.').trim().to_lowercase();
    match said.as_str() {
        "beat" | "another beat" | "a beat" => Some(Pause::Beat),
        "long silence" | "long pause" => Some(Pause::LongSilence),
        "silence" | "a pause" | "pause" => Some(Pause::Silence),
        _ => None,
    }
}

/// The words without markdown emphasis. Only the markers go; nothing else in
/// the line is touched.
fn strip_markup(text: &str) -> String {
    let mut out = text.replace("**", "");
    out = out.trim().to_string();
    // A lone leading and trailing asterisk is italics.
    if out.len() > 2 && out.starts_with('*') && out.ends_with('*') {
        out = out[1..out.len() - 1].trim().to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scene 4's opening, as written in the series script (CRLF and the
    /// two trailing spaces after each cue included).
    const SCENE_4: &str = "# EPISODE 1 \u{2014} NAYE PADOSI\r\n\r\n**Setting:** Chirailiya, Bihar  \r\n\r\n---\r\n\r\n## SCENE 4 \u{2014} THE THREE BEROZGAARS\r\n\r\n**EXT. CHHOTU'S TEA STALL \u{2014} MORNING**\r\n\r\nTin roof.\r\n\r\nWooden bench.\r\n\r\nChhotu serves them.\r\n\r\n**NARRATOR (V.O.)**  \r\nGuddu.\r\n\r\nGuddu folds yesterday's newspaper like a minister preparing a statement.\r\n\r\n**GUDDU**  \r\nMain soch raha hoon kuch bada karun.\r\n\r\n**MUNNA**  \r\nKal bhi soch rahe the.\r\n\r\nLalten sips tea.\r\n\r\n---\r\n\r\n**NARRATOR (V.O.)**  \r\nMunna.\r\n\r\nBeat.\r\n\r\n**NARRATOR (V.O.)**  \r\nTeeno jhooth hain.\r\n\r\n---\r\n\r\n## SCENE 5 \u{2014} RAJU ARRIVES\r\n\r\nRAJU, 28, walks briskly toward them.\r\n\r\n**RAJU**  \r\nMain sarkar mein kaam karta hoon. Sarkar nahi hoon.\r\n\r\n---\r\n\r\n# END\r\n\r\n---\r\n\r\n## MAIN COMEDY BEATS BUILT INTO THE PILOT\r\n\r\n1. Crow dismissing Chirailiya history.\r\n";

    fn scene_4() -> Screenplay {
        Screenplay::parse(SCENE_4)
    }

    #[test]
    fn the_title_and_the_scenes_are_found() {
        let play = scene_4();
        assert_eq!(play.title, "EPISODE 1 \u{2014} NAYE PADOSI");
        assert_eq!(play.scenes.len(), 2, "the notes after # END are not scenes");
        assert_eq!(play.scenes[0].number, 4);
        assert_eq!(play.scenes[0].title, "THE THREE BEROZGAARS");
        assert_eq!(play.scenes[1].number, 5);
        assert!(play.scene(5).is_some());
        assert!(play.scene(6).is_none());
    }

    #[test]
    fn a_slugline_is_split_into_place_location_and_time() {
        let play = scene_4();
        let slug = play.scenes[0].slugline.as_ref().expect("a slugline");
        assert_eq!(slug.place, Place::Exterior);
        assert_eq!(slug.location, "CHHOTU'S TEA STALL");
        assert_eq!(slug.time, "MORNING");
        // The slugline is not also an item.
        assert!(
            play.scenes[0]
                .items
                .iter()
                .all(|i| !matches!(i, Item::Slugline(_)))
        );
        // Scene 5 has none of its own.
        assert!(play.scenes[1].slugline.is_none());
    }

    #[test]
    fn an_interior_slugline_is_interior() {
        let play = Screenplay::parse(
            "## SCENE 7 \u{2014} AMMA\n\n**INT. RAJU'S HOUSE \u{2014} EARLIER THAT MORNING**\n\nQuick insert.\n",
        );
        let slug = play.scenes[0].slugline.as_ref().expect("a slugline");
        assert_eq!(slug.place, Place::Interior);
        assert_eq!(slug.location, "RAJU'S HOUSE");
        assert_eq!(slug.time, "EARLIER THAT MORNING");
    }

    #[test]
    fn cues_carry_their_lines_and_how_they_are_heard() {
        let play = scene_4();
        let dialogue: Vec<&Dialogue> = play.scenes[0]
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Dialogue(d) => Some(d),
                _ => None,
            })
            .collect();
        assert_eq!(dialogue.len(), 5);
        assert_eq!(dialogue[0].speaker, "NARRATOR");
        assert_eq!(dialogue[0].voice, Voice::VoiceOver);
        assert_eq!(dialogue[0].text, "Guddu.");
        assert_eq!(dialogue[1].speaker, "GUDDU");
        assert_eq!(dialogue[1].voice, Voice::OnScreen);
        assert_eq!(dialogue[1].text, "Main soch raha hoon kuch bada karun.");
        assert_eq!(dialogue[2].speaker, "MUNNA");
    }

    #[test]
    fn off_screen_cues_and_multi_word_names_are_read() {
        let play = Screenplay::parse(
            "## SCENE 6 \u{2014} X\n\n**SHARMA JI (O.S.)**  \nKya?\n\n**MAN 2 (O.S.)**\nScale le aao!\n\n**VERMA JI**\nMain pehle se bol raha hoon.\n",
        );
        let items = &play.scenes[0].items;
        let Item::Dialogue(a) = &items[0] else {
            panic!("{items:?}")
        };
        assert_eq!((a.speaker.as_str(), a.voice), ("SHARMA JI", Voice::OffScreen));
        let Item::Dialogue(b) = &items[1] else {
            panic!("{items:?}")
        };
        assert_eq!((b.speaker.as_str(), b.voice), ("MAN 2", Voice::OffScreen));
        let Item::Dialogue(c) = &items[2] else {
            panic!("{items:?}")
        };
        assert_eq!((c.speaker.as_str(), c.voice), ("VERMA JI", Voice::OnScreen));
    }

    #[test]
    fn a_cue_with_several_lines_joins_them() {
        let play = Screenplay::parse("## SCENE 1 \u{2014} X\n\n**GUDDU**\nEk line.\nDusri line.\n\nAction.\n");
        let Item::Dialogue(d) = &play.scenes[0].items[0] else {
            panic!("{:?}", play.scenes[0].items)
        };
        assert_eq!(d.text, "Ek line. Dusri line.");
        assert_eq!(play.scenes[0].items[1], Item::Action("Action.".into()));
    }

    #[test]
    fn beats_breaks_and_pauses_are_their_own_items() {
        let play = scene_4();
        let items = &play.scenes[0].items;
        assert!(items.contains(&Item::Break), "a --- is a beat break");
        assert!(
            items.contains(&Item::Pause(Pause::Beat)),
            "Beat. is a pause: {items:?}"
        );
    }

    #[test]
    fn every_pause_wording_is_recognised() {
        let play = Screenplay::parse(
            "## SCENE 1 \u{2014} X\n\nBeat.\n\nAnother beat.\n\nLong silence.\n\nSilence.\n\nA pause.\n",
        );
        let pauses: Vec<Pause> = play.scenes[0]
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Pause(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(
            pauses,
            vec![
                Pause::Beat,
                Pause::Beat,
                Pause::LongSilence,
                Pause::Silence,
                Pause::Silence
            ]
        );
    }

    #[test]
    fn action_is_everything_else_in_order() {
        let play = scene_4();
        let actions: Vec<&str> = play.scenes[0]
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Action(a) => Some(a.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(actions[0], "Tin roof.");
        assert_eq!(actions[1], "Wooden bench.");
        assert_eq!(actions[2], "Chhotu serves them.");
        assert!(actions.contains(&"Lalten sips tea."));
        // The cast introduction in capitals is action too.
        assert_eq!(
            play.scenes[1].items[0],
            Item::Action("RAJU, 28, walks briskly toward them.".into())
        );
    }

    /// A sign is bold and upper-case like a cue, but has nothing to say.
    #[test]
    fn a_bold_line_with_no_dialogue_is_action_not_a_cue() {
        let play = Screenplay::parse(
            "## SCENE 3 \u{2014} SHANTI COLONY\n\n### SHANTI COLONY\n\n**BIKAU HAI**\n\n**NARRATOR (V.O.)**\nAur ye tha Mishra babu ka purana ghar.\n\nBeat.\n\n**NARRATOR (V.O.)**\nTeen saal se bik raha tha.\n\nAnother beat.\n\n**NARRATOR (V.O.)**\nGhar nahi. Board.\n\n**CUT TO BLACK.**\n",
        );
        let items = &play.scenes[0].items;
        assert_eq!(items[0], Item::Action("SHANTI COLONY".into()));
        assert_eq!(items[1], Item::Action("BIKAU HAI".into()));
        assert!(matches!(&items[2], Item::Dialogue(d) if d.speaker == "NARRATOR"));
        assert_eq!(items[3], Item::Pause(Pause::Beat));
        assert_eq!(items[5], Item::Pause(Pause::Beat));
        assert_eq!(items.last(), Some(&Item::Action("CUT TO BLACK.".into())));
        // The sign is not a speaker.
        assert!(
            !items
                .iter()
                .any(|i| matches!(i, Item::Dialogue(d) if d.speaker == "BIKAU HAI"))
        );
    }

    #[test]
    fn nothing_after_the_end_marker_is_read() {
        let play = scene_4();
        let everything = format!("{:?}", play.scenes);
        assert!(!everything.contains("MAIN COMEDY"), "the notes leaked in");
        assert!(!everything.contains("Crow dismissing"));
    }

    #[test]
    fn a_second_slugline_is_a_cut_inside_the_scene() {
        let play = Screenplay::parse(
            "## SCENE 9 \u{2014} X\n\n**EXT. ROAD \u{2014} DAY**\n\nA truck.\n\n**INT. CAB \u{2014} DAY**\n\nA driver.\n",
        );
        let scene = &play.scenes[0];
        assert_eq!(scene.slugline.as_ref().unwrap().location, "ROAD");
        assert!(
            scene
                .items
                .iter()
                .any(|i| matches!(i, Item::Slugline(s) if s.location == "CAB"))
        );
    }

    #[test]
    fn a_script_with_no_scenes_is_empty_not_an_error() {
        let play = Screenplay::parse("just some words\n");
        assert!(play.scenes.is_empty());
    }
}
