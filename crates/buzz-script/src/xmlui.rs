//! **XMLUI**: Animate's small dialogs, described in XML (`fl.xmlPanel`,
//! `fl.xmlPanelFromString`) -- JSFL_COMMANDS_PLAN J6.
//!
//! A command that wants a few settings writes a `<dialog>` of labels, text
//! boxes, check boxes, drop-downs and radio groups, and gets back an object with
//! `dismiss` (`"accept"` or `"cancel"`) and one string per control id. This
//! reads the XML into a [`Form`] the host can draw as a real dialog; the
//! answers come back as `(id, value)` pairs.
//!
//! Only what such a dialog is made of is read. Layout tags (`vbox`, `hbox`,
//! `grid`, ...) are walked through, and a control BuzzAnimate does not know is
//! skipped rather than failing the dialog.

/// What kind of control one line of the dialog is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    /// Static text (`<label>`).
    Label,
    /// `<textbox>` (and a colour chip, which is a `#RRGGBB` text).
    Text,
    /// `<popupslider>` / `<slider>`: a number between `min` and `max`.
    Number,
    /// `<checkbox>`: the value is `"true"` or `"false"`.
    Checkbox,
    /// `<menulist>` / `<listbox>`: one of `options`, shown as a drop-down.
    Dropdown,
    /// `<radiogroup>`: one of `options`, shown as buttons.
    Radio,
}

/// One control of a dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    /// The name the script reads the answer under. Empty for static text.
    pub id: String,
    pub kind: ControlKind,
    /// The caption.
    pub label: String,
    /// The value it starts with.
    pub value: String,
    /// `(label, value)` of each choice, for a drop-down or radio group.
    pub options: Vec<(String, String)>,
    pub min: f64,
    pub max: f64,
}

/// A whole dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub title: String,
    /// The buttons, `"accept"` and/or `"cancel"`.
    pub buttons: Vec<String>,
    pub controls: Vec<Control>,
}

impl Form {
    /// The answer a dialog gives with nobody to ask: every control as it starts.
    pub fn defaults(&self) -> Vec<(String, String)> {
        self.controls
            .iter()
            .filter(|c| !c.id.is_empty() && c.kind != ControlKind::Label)
            .map(|c| (c.id.clone(), c.value.clone()))
            .collect()
    }
}

/// One tag of the document.
struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
    closing: bool,
    self_closing: bool,
}

impl Tag {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#10;", "\n")
        .replace("&amp;", "&")
}

fn tags(xml: &str) -> Vec<Tag> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map(|e| &after[e + 3..]).unwrap_or("");
            continue;
        }
        // An attribute value may hold a `>`, so the end is the first `>`
        // outside quotes.
        let mut quote: Option<char> = None;
        let mut end = None;
        for (i, ch) in rest.char_indices().skip(1) {
            match (quote, ch) {
                (Some(q), c) if c == q => quote = None,
                (None, '"') | (None, '\'') => quote = Some(ch),
                (None, '>') => {
                    end = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        let body = &rest[1..end];
        rest = &rest[end + 1..];
        if body.starts_with('?') || body.starts_with('!') {
            continue;
        }
        let closing = body.starts_with('/');
        let self_closing = body.ends_with('/');
        let body = body.trim_start_matches('/').trim_end_matches('/').trim();
        let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
        let name = body[..name_end].to_ascii_lowercase();
        let mut attrs = Vec::new();
        let mut a = body[name_end..].trim_start();
        while !a.is_empty() {
            let Some(eq) = a.find('=') else { break };
            let key = a[..eq].trim().to_string();
            let after = a[eq + 1..].trim_start();
            let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                break;
            };
            let Some(close) = after[1..].find(q) else { break };
            attrs.push((key, unescape(&after[1..1 + close])));
            a = after[close + 2..].trim_start();
        }
        out.push(Tag { name, attrs, closing, self_closing });
    }
    out
}

fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

/// Read an XMLUI document. `None` if it has no `<dialog>`.
pub fn parse(xml: &str) -> Option<Form> {
    let mut form: Option<Form> = None;
    // Index into `form.controls` of the open drop-down / radio group.
    let mut open_choice: Option<usize> = None;
    for tag in tags(xml) {
        if tag.name == "dialog" && !tag.closing {
            let buttons = tag
                .attr("buttons")
                .map(|b| {
                    b.split(',')
                        .map(|x| x.trim().to_ascii_lowercase())
                        .filter(|x| x == "accept" || x == "cancel")
                        .collect::<Vec<_>>()
                })
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| vec!["accept".into(), "cancel".into()]);
            form = Some(Form {
                title: tag.attr("title").unwrap_or("").to_string(),
                buttons,
                controls: Vec::new(),
            });
            continue;
        }
        let Some(f) = form.as_mut() else { continue };
        if tag.closing {
            if matches!(tag.name.as_str(), "menulist" | "listbox" | "radiogroup") {
                open_choice = None;
            }
            continue;
        }
        let id = tag.attr("id").unwrap_or("").to_string();
        let label = tag.attr("label").unwrap_or("").to_string();
        let new = |kind, value: String| Control {
            id: id.clone(),
            kind,
            label: label.clone(),
            value,
            options: Vec::new(),
            min: 0.0,
            max: 100.0,
        };
        match tag.name.as_str() {
            "label" => {
                let mut c = new(ControlKind::Label, String::new());
                c.label = tag.attr("value").unwrap_or(&label).to_string();
                f.controls.push(c);
            }
            "textbox" | "colorchip" => {
                f.controls
                    .push(new(ControlKind::Text, tag.attr("value").unwrap_or("").to_string()));
            }
            "checkbox" => {
                let on = truthy(tag.attr("checked"));
                f.controls.push(new(ControlKind::Checkbox, on.to_string()));
            }
            "popupslider" | "slider" => {
                let mut c = new(ControlKind::Number, tag.attr("value").unwrap_or("0").to_string());
                c.min = tag.attr("minvalue").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                c.max = tag.attr("maxvalue").and_then(|v| v.parse().ok()).unwrap_or(100.0);
                f.controls.push(c);
            }
            "menulist" | "listbox" | "radiogroup" => {
                let kind = if tag.name == "radiogroup" { ControlKind::Radio } else { ControlKind::Dropdown };
                f.controls.push(new(kind, String::new()));
                open_choice = if tag.self_closing { None } else { Some(f.controls.len() - 1) };
            }
            "menuitem" | "listitem" | "radio" => {
                if let Some(i) = open_choice {
                    let c = &mut f.controls[i];
                    let shown = tag.attr("label").unwrap_or("").to_string();
                    let value = tag.attr("value").unwrap_or(&shown).to_string();
                    if truthy(tag.attr("selected")) || c.options.is_empty() && c.value.is_empty() {
                        c.value = value.clone();
                    }
                    c.options.push((shown, value));
                }
            }
            _ => {}
        }
    }
    form
}

/// The object a script receives: `{dismiss: "accept"|"cancel", <id>: "<value>", ...}`
/// as JSON text. `answers` is `None` for a dialog that was cancelled.
pub fn result_json(answers: Option<&[(String, String)]>) -> String {
    let mut map = serde_json::Map::new();
    map.insert(
        "dismiss".into(),
        serde_json::Value::String(if answers.is_some() { "accept" } else { "cancel" }.into()),
    );
    for (id, value) in answers.unwrap_or(&[]) {
        map.insert(id.clone(), serde_json::Value::String(value.clone()));
    }
    serde_json::Value::Object(map).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0"?>
<dialog title="Extend" buttons="accept, cancel">
  <!-- a comment with <tags> in it -->
  <vbox>
    <label value="How long?"/>
    <textbox id="secs" label="Seconds" value="185"/>
    <checkbox id="loop" label="Loop it" checked="true"/>
    <menulist id="mode" label="Mode">
      <menupop>
        <menuitem label="Hold" value="hold"/>
        <menuitem label="Loop" value="loop" selected="true"/>
      </menupop>
    </menulist>
    <radiogroup id="side">
      <radio label="Left" value="l"/>
      <radio label="Right" value="r"/>
    </radiogroup>
    <popupslider id="amount" minvalue="1" maxvalue="9" value="4"/>
  </vbox>
</dialog>"#;

    #[test]
    fn a_dialog_is_read_into_its_controls() {
        let f = parse(XML).expect("a dialog");
        assert_eq!(f.title, "Extend");
        assert_eq!(f.buttons, ["accept", "cancel"]);
        let kinds: Vec<_> = f.controls.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            [
                ControlKind::Label,
                ControlKind::Text,
                ControlKind::Checkbox,
                ControlKind::Dropdown,
                ControlKind::Radio,
                ControlKind::Number
            ]
        );
        assert_eq!(f.controls[0].label, "How long?");
        assert_eq!(
            f.defaults(),
            [
                ("secs".to_string(), "185".to_string()),
                ("loop".into(), "true".into()),
                ("mode".into(), "loop".into()),
                ("side".into(), "l".into()),
                ("amount".into(), "4".into()),
            ]
        );
        assert_eq!(f.controls[5].min, 1.0);
        assert_eq!(f.controls[5].max, 9.0);
    }

    #[test]
    fn the_answer_object_carries_dismiss_and_the_values() {
        let r = result_json(Some(&[("secs".into(), "9".into())]));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["dismiss"], "accept");
        assert_eq!(v["secs"], "9");
        let c: serde_json::Value = serde_json::from_str(&result_json(None)).unwrap();
        assert_eq!(c["dismiss"], "cancel");
    }

    #[test]
    fn text_without_a_dialog_is_not_one() {
        assert!(parse("<hbox><label value='x'/></hbox>").is_none());
    }
}
