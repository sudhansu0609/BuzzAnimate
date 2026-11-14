//! Temporary: import an Animate keymap into a scratch workspace and print what happened.
fn main() {
    let path = std::env::args().nth(1).expect("keymap xml");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut ws = buzz_ui::Workspace::animate();
    let b = buzz_ui::animate_keys::parse(&text);
    let r = buzz_ui::animate_keys::apply(&mut ws, &b, buzz_app::app::KEYBOARD_COMMANDS);
    println!("commands {} scripts {}", r.commands, r.scripts);
    println!("UNBOUND: {:#?}", r.unbound);
    println!("CONFLICTS: {:#?}", r.conflicts);
    println!("UNMAPPED ({}): {:?}", r.unmapped.len(), r.unmapped);
    for (k, v) in &ws.script_keys { println!("script {k:<40} {:?}", v.iter().map(|c| c.label()).collect::<Vec<_>>()); }
    for (k, v) in &ws.keymap { println!("keymap {k:<40} {:?}", v.as_ref().map(|c| c.label())); }
    for (k, v) in &ws.extra_keys { println!("extra  {k:<40} {:?}", v.iter().map(|c| c.label()).collect::<Vec<_>>()); }
}
