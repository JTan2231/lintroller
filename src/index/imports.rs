use std::collections::{BTreeMap, BTreeSet, HashSet};

/// File-level evidence from all compiler records for a source. The index can
/// contain several targets or configurations for the same file.
#[derive(Default)]
pub struct ImportUsage {
    pub current_modules: HashSet<String>,
    pub referenced_modules: HashSet<String>,
    pub module_locations: BTreeMap<String, BTreeSet<(u32, u32)>>,
    pub unknown_owner: bool,
}

impl ImportUsage {
    pub fn declared_in(&mut self, module: &str) {
        remember(&mut self.current_modules, module);
    }

    pub fn record(&mut self, usr: &[u8], kind: u32, name: &[u8], line: u32, column: u32) {
        if kind == 1 {
            if let Ok(name) = std::str::from_utf8(name) {
                let module = name.split('.').next().unwrap_or(name);
                if let Some(locations) = self.module_locations.get_mut(module) {
                    locations.insert((line, column));
                } else {
                    self.module_locations
                        .insert(module.to_owned(), BTreeSet::from([(line, column)]));
                }
            }
            return;
        }
        let Some(mut mangled) = usr.strip_prefix(b"s:") else {
            // Clang USRs identify the declaration but generally omit its exporting module.
            self.unknown_owner = true;
            return;
        };
        if mangled.starts_with(b"So") || mangled.starts_with(b"SC") {
            self.unknown_owner = true;
        } else if mangled.first().is_some_and(u8::is_ascii_digit) {
            let Some((module, rest)) = identifier(mangled) else {
                self.unknown_owner = true;
                return;
            };
            remember(&mut self.referenced_modules, module);
            mangled = rest;
        } else if !mangled.starts_with(b"s") && !mangled.starts_with(b"S") {
            self.unknown_owner = true;
        }

        // A cross-module extension has a second module in its declaration
        // context. Walk length-prefixed identifiers so an 'E' inside an
        // identifier is never taken for the extension operator.
        while let Some((&first, rest)) = mangled.split_first() {
            if first.is_ascii_digit() {
                if let Some((word, tail)) = identifier(mangled) {
                    if tail.starts_with(b"E") {
                        remember(&mut self.referenced_modules, word);
                        mangled = &tail[1..];
                    } else {
                        mangled = tail;
                    }
                    continue;
                }
            }
            // A generic signature or a substituted module can precede E.
            // Its owner requires full demangling. SE is the standard
            // substitution for Encodable.
            if first == b'E' {
                self.unknown_owner = true;
            }
            mangled = if first == b'S' && rest.starts_with(b"E") {
                &rest[1..]
            } else {
                rest
            };
        }
    }
}

fn remember(modules: &mut HashSet<String>, module: &str) {
    if !module.is_empty() && !modules.contains(module) {
        modules.insert(module.to_owned());
    }
}

fn identifier(input: &[u8]) -> Option<(&str, &[u8])> {
    // Leading zero is Swift's encoding for identifiers with word
    // substitutions or Unicode mangling. It isn't a literal byte count.
    if input.first() == Some(&b'0') {
        return None;
    }
    let end = input.iter().position(|b| !b.is_ascii_digit())?;
    let length: usize = std::str::from_utf8(input.get(..end)?).ok()?.parse().ok()?;
    let (bytes, rest) = input[end..].split_at_checked(length)?;
    let word = std::str::from_utf8(bytes).ok()?;
    Some((word, rest))
}
