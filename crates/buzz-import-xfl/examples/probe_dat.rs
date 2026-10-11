//! Probe the `bin/*.dat` bitmap containers inside an .fla (zip) package.
use std::io::{Cursor, Read};

fn main() {
    let path = std::env::args().nth(1).expect("usage: probe_dat <file.fla>");
    let bytes = std::fs::read(&path).unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap();
        if !entry.name().starts_with("bin/") || !entry.name().ends_with(".dat") {
            continue;
        }
        let name = entry.name().to_string();
        let short: String = name
            .rsplit('/')
            .next()
            .unwrap_or(&name)
            .chars()
            .take(6)
            .collect();
        let mut raw = Vec::new();
        entry.read_to_end(&mut raw).unwrap();

        if raw.len() < 32 {
            println!("== {short} too small");
            continue;
        }
        // Header model: u16@4 width, u16@6 height (stored/downsampled size)
        let w = u16::from_le_bytes([raw[4], raw[5]]) as usize;
        let h = u16::from_le_bytes([raw[6], raw[7]]) as usize;
        let expected = w * h * 4;

        // Try candidate start offsets for a raw deflate stream (0..4KB covers both variants).
        let range: usize = raw.len().min(4096);
        // Also try zlib (header-prefixed) decode at 28.
        let mut found: Option<(usize, Vec<u8>)> = None;
        {
            let mut dec = flate2::read::ZlibDecoder::new(Cursor::new(&raw[28..]));
            let mut out = Vec::new();
            if dec.read_to_end(&mut out).is_ok() && out.len() >= expected {
                found = Some((28, out));
            }
        }
        if found.is_none() {
            'outer: for skip in 0..range {
                let mut dec = flate2::read::DeflateDecoder::new(Cursor::new(&raw[skip..]));
                let mut out = Vec::new();
                match dec.read_to_end(&mut out) {
                    Ok(_) if out.len() >= expected => {
                        found = Some((skip, out));
                        break 'outer;
                    }
                    _ => {}
                }
            }
        }

        match found {
            Some((skip, out)) => {
                println!(
                    "== {short} stored {w}x{h}, expect {expected}: inflate@{skip} OK -> {} bytes",
                    out.len()
                );
                // channel stats over the first w*h*4 bytes
                let n = out.len().min(expected);
                if n >= 16 {
                    let mut sums = [0u64; 4];
                    let mut ffs = [0u64; 4];
                    let mut zeros = [0u64; 4];
                    for p in 0..n / 4 {
                        for c in 0..4 {
                            let v = out[p * 4 + c] as u64;
                            sums[c] += v;
                            if v == 255 { ffs[c] += 1 }
                            if v == 0 { zeros[c] += 1 }
                        }
                    }
                    let px = (n / 4) as u64;
                    for c in 0..4 {
                        println!(
                            "    ch{c}: mean {:.1} ff {}% zero {}%",
                            sums[c] as f64 / px as f64,
                            ffs[c] * 100 / px,
                            zeros[c] * 100 / px
                        );
                    }
                }
            }
            None => {
                let dump: String = raw
                    .iter()
                    .take(96)
                    .map(|b| format!("{b:02x} "))
                    .collect();
                println!(
                    "== {short} stored {}x{}, expect {}: NO single-stream inflate in 0..{} (raw len {}) \n    header: {}",
                    w, h, expected, range, raw.len(), dump
                );

                // Diagnostic: is @28 a zlib stream? Print error / partial length.
                let mut dec = flate2::read::ZlibDecoder::new(Cursor::new(&raw[28..]));
                let mut out = Vec::new();
                match dec.read_to_end(&mut out) {
                    Ok(_) => println!("    zlib@28 partial OK -> {} bytes", out.len()),
                    Err(e) => println!("    zlib@28 err: {e} (partial {} bytes)", out.len()),
                }

                // Walk concatenated streams using the exact ZlibDecoder read loop:
                // chunked 32KB reads from a BufReader; after Ok(0) (StreamEnd),
                // peek remaining input and start a fresh decoder there.
                use std::io::{BufReader, Read};
                for tag in ["zlib@28", "raw@30"] {
                    let start = if tag.starts_with("zlib") { 28 } else { 30 };
                    println!("    --- walk {tag}");
                    let mut pos = start;
                    let mut total = Vec::new();
                    let mut streams = 0u32;
                    while pos < raw.len() && total.len() <= expected + (1 << 20) {
                        let mut consumed = 0u64; // exact bytes eaten by this stream's decoder
                        if tag.starts_with("zlib") {
                            let mut dec = flate2::bufread::ZlibDecoder::new(BufReader::new(
                                Cursor::new(&raw[pos..]),
                            ));
                            loop {
                                let mut chunk = vec![0u8; 32 * 1024];
                                match dec.read(&mut chunk) {
                                    Ok(0) => {
                                        consumed = dec.total_in(); // includes zlib hdr + adler32
                                        break;
                                    }
                                    Ok(n) => total.extend_from_slice(&chunk[..n]),
                                    Err(e) => {
                                        println!(
                                            "    err in stream {}: {e} (total {} / {} bytes)",
                                            streams + 1,
                                            total.len(),
                                            expected
                                        );
                                        pos = raw.len(); // stop walking
                                        break;
                                    }
                                }
                            }
                        } else {
                            let mut dec = flate2::bufread::DeflateDecoder::new(BufReader::new(
                                Cursor::new(&raw[pos..]),
                            ));
                            loop {
                                let mut chunk = vec![0u8; 32 * 1024];
                                match dec.read(&mut chunk) {
                                    Ok(0) => {
                                        consumed = dec.total_in(); // raw deflate: no hdr/adler
                                        break;
                                    }
                                    Ok(n) => total.extend_from_slice(&chunk[..n]),
                                    Err(e) => {
                                        println!(
                                            "    err in stream {}: {e} (total {} / {} bytes)",
                                            streams + 1,
                                            total.len(),
                                            expected
                                        );
                                        pos = raw.len(); // stop walking
                                        break;
                                    }
                                }
                            }
                        }
                        if consumed == 0 && pos == start {
                            break; // nothing decoded (error above set pos=raw.len)
                        }
                        pos += consumed as usize;
                        streams += 1;
                        println!(
                            "    stream {} ended at input @{} (+{} bytes, total {} / {}) next: {}",
                            streams,
                            pos,
                            consumed,
                            total.len(),
                            expected,
                            hexdump(&raw[pos..(pos + 32).min(raw.len())])
                        );
                    }
                    if total.len() >= expected {
                        println!(
                            "    walk {tag}: REACHED expected after {} streams: {} / {} bytes",
                            streams,
                            total.len(),
                            expected
                        );
                    } else {
                        println!("    walk {tag} stopped at input @{} with {} / {} bytes", pos, total.len(), expected);
                    }
                }
            }
        }
    }

    // DOMBitmapItem / BitmapFill context from DOMDocument.xml.
    if let Ok(mut e) = zip.by_name("DOMDocument.xml") {
        let mut xml = String::new();
        e.read_to_string(&mut xml).unwrap();
        println!("== DOMBitmapItem samples:");
        for m in occurrences(&xml, "<DOMBitmapItem").into_iter().take(8) {
            println!("  {m}");
        }
        println!("== BitmapFill samples:");
        for m in occurrences(&xml, "<BitmapFill").into_iter().take(3) {
            println!("  {m}");
        }
    }
}

fn hexdump(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x} ")).collect()
}

fn occurrences(haystack: &str, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, c) in haystack.char_indices() {
        if c == '<' && haystack[i..].starts_with(tag) {
            let end = haystack[i..]
                .find('>')
                .map(|e| i + e + 1)
                .unwrap_or(haystack.len());
            out.push(haystack[i..end].to_string());
        }
    }
    out
}
