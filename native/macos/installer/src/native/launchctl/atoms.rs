//! Rewrites `launchctl list` output into an `OpenStep` plist the plist reader accepts.
//!
//! The status dictionary uses bare `mach-port-object`, `true` and `false` atoms and
//! `;` array separators, none of which are plist syntax. Each atom outside strings and
//! comments becomes a quoted marker, and the status parser later requires every marker
//! to appear only where launchctl puts it.

use crate::native::NativeError;

pub(super) const MACH_PORT_OBJECT_MARKER: &str = "__MIXRELAY_LAUNCHCTL_MACH_PORT_OBJECT__";
pub(super) const TRUE_MARKER: &str = "__MIXRELAY_LAUNCHCTL_TRUE__";
pub(super) const FALSE_MARKER: &str = "__MIXRELAY_LAUNCHCTL_FALSE__";

/// How many of each bare atom were quoted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct LaunchctlAtoms {
    pub(super) mach_port: usize,
    pub(super) true_values: usize,
    pub(super) false_values: usize,
}

#[derive(Clone, Copy)]
enum Atom {
    MachPort,
    True,
    False,
}

impl Atom {
    const ALL: [(&'static [u8], Self); 3] = [
        (b"mach-port-object", Self::MachPort),
        (b"true", Self::True),
        (b"false", Self::False),
    ];

    fn marker(self) -> &'static str {
        match self {
            Self::MachPort => MACH_PORT_OBJECT_MARKER,
            Self::True => TRUE_MARKER,
            Self::False => FALSE_MARKER,
        }
    }
}

impl LaunchctlAtoms {
    fn record(&mut self, atom: Atom) {
        match atom {
            Atom::MachPort => self.mach_port += 1,
            Atom::True => self.true_values += 1,
            Atom::False => self.false_values += 1,
        }
    }
}

/// Quotes the launchctl status atoms that are not `OpenStep` plist values.
pub(super) fn quote_launchctl_atoms(
    bytes: &[u8],
) -> Result<(Vec<u8>, LaunchctlAtoms), NativeError> {
    let mut scanner = Scanner::new(bytes);
    while scanner.index < bytes.len() {
        match scanner.context {
            Context::Code => scanner.scan_code(),
            Context::Quoted { escaped } => scanner.copy_quoted(escaped),
            Context::LineComment => scanner.copy_line_comment(),
            Context::BlockComment => scanner.copy_block_comment(),
        }
    }
    if matches!(
        scanner.context,
        Context::Quoted { .. } | Context::BlockComment
    ) {
        return Err(NativeError::LaunchdState(
            "exact-label result has unterminated string or comment",
        ));
    }
    Ok((scanner.output, scanner.atoms))
}

#[derive(Clone, Copy)]
enum Context {
    Code,
    Quoted { escaped: bool },
    LineComment,
    BlockComment,
}

struct Scanner<'a> {
    bytes: &'a [u8],
    index: usize,
    output: Vec<u8>,
    atoms: LaunchctlAtoms,
    context: Context,
    dictionary_depth: usize,
    /// The dictionary depth at which each open array started.
    array_dictionary_depths: Vec<usize>,
}

impl<'a> Scanner<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            index: 0,
            output: Vec::with_capacity(bytes.len() + 32),
            atoms: LaunchctlAtoms::default(),
            context: Context::Code,
            dictionary_depth: 0,
            array_dictionary_depths: Vec::new(),
        }
    }

    fn copy(&mut self, count: usize) {
        self.output
            .extend_from_slice(&self.bytes[self.index..self.index + count]);
        self.index += count;
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.bytes[self.index..].starts_with(prefix)
    }

    fn copy_quoted(&mut self, escaped: bool) {
        let current = self.bytes[self.index];
        self.copy(1);
        self.context = if escaped {
            Context::Quoted { escaped: false }
        } else if current == b'\\' {
            Context::Quoted { escaped: true }
        } else if current == b'"' {
            Context::Code
        } else {
            Context::Quoted { escaped: false }
        };
    }

    fn copy_line_comment(&mut self) {
        let current = self.bytes[self.index];
        self.copy(1);
        if current == b'\n' {
            self.context = Context::Code;
        }
    }

    fn copy_block_comment(&mut self) {
        if self.starts_with(b"*/") {
            self.copy(2);
            self.context = Context::Code;
        } else {
            self.copy(1);
        }
    }

    fn scan_code(&mut self) {
        let current = self.bytes[self.index];
        let opened = if current == b'"' {
            Some((Context::Quoted { escaped: false }, 1))
        } else if self.starts_with(b"//") {
            Some((Context::LineComment, 2))
        } else if self.starts_with(b"/*") {
            Some((Context::BlockComment, 2))
        } else {
            None
        };
        if let Some((context, length)) = opened {
            self.context = context;
            self.copy(length);
            return;
        }
        if self.consume_array_separator(current) {
            return;
        }
        match self.atom_here() {
            Some((length, atom)) => self.quote_atom(length, atom),
            None => self.copy(1),
        }
    }

    fn quote_atom(&mut self, length: usize, atom: Atom) {
        self.atoms.record(atom);
        self.output.push(b'"');
        self.output.extend_from_slice(atom.marker().as_bytes());
        self.output.push(b'"');
        self.index += length;
    }

    /// Tracks nesting and rewrites `;` between array items as `,`, dropping a trailing one.
    fn consume_array_separator(&mut self, current: u8) -> bool {
        match current {
            b'{' => self.dictionary_depth = self.dictionary_depth.saturating_add(1),
            b'}' => self.dictionary_depth = self.dictionary_depth.saturating_sub(1),
            b'(' => self.array_dictionary_depths.push(self.dictionary_depth),
            b')' => {
                self.array_dictionary_depths.pop();
            }
            b';' if self.array_dictionary_depths.last() == Some(&self.dictionary_depth) => {
                let next = self.bytes[self.index + 1..]
                    .iter()
                    .copied()
                    .find(|byte| !byte.is_ascii_whitespace());
                if next != Some(b')') {
                    self.output.push(b',');
                }
                self.index += 1;
                return true;
            }
            _ => {}
        }
        false
    }

    fn atom_here(&self) -> Option<(usize, Atom)> {
        let before = self
            .index
            .checked_sub(1)
            .and_then(|before| self.bytes.get(before));
        Atom::ALL.into_iter().find_map(|(text, atom)| {
            let end = self.index + text.len();
            (self.bytes.get(self.index..end) == Some(text)
                && !is_atom_byte(before)
                && !is_atom_byte(self.bytes.get(end)))
            .then_some((text.len(), atom))
        })
    }
}

fn is_atom_byte(byte: Option<&u8>) -> bool {
    byte.is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quoted(input: &str) -> (String, LaunchctlAtoms) {
        let (bytes, atoms) = quote_launchctl_atoms(input.as_bytes()).expect("balanced input");
        (String::from_utf8(bytes).expect("ASCII output"), atoms)
    }

    #[test]
    fn bare_atoms_become_markers_but_quoted_and_commented_text_is_kept() {
        let (text, atoms) = quoted(
            r#"{"a"=true;"b"=false;"m"={"s"=mach-port-object;};"q"="true";// true
/* false */}"#,
        );
        assert_eq!(
            text,
            format!(
                r#"{{"a"="{TRUE_MARKER}";"b"="{FALSE_MARKER}";"m"={{"s"="{MACH_PORT_OBJECT_MARKER}";}};"q"="true";// true
/* false */}}"#
            )
        );
        assert_eq!(
            atoms,
            LaunchctlAtoms {
                mach_port: 1,
                true_values: 1,
                false_values: 1
            }
        );
    }

    #[test]
    fn atoms_inside_longer_words_or_after_escaped_quotes_are_not_rewritten() {
        let (text, atoms) = quoted(r#"{"a"=untrue;"b"=true-ish;"c"="\"true";}"#);
        assert_eq!(text, r#"{"a"=untrue;"b"=true-ish;"c"="\"true";}"#);
        assert_eq!(atoms, LaunchctlAtoms::default());
    }

    #[test]
    fn array_separators_become_commas_and_the_trailing_one_is_dropped() {
        let (text, _) = quoted(r#"{"p"=("a";"b"; );"d"={"k"="v";};}"#);
        assert_eq!(text, r#"{"p"=("a","b" );"d"={"k"="v";};}"#);
    }

    #[test]
    fn unterminated_strings_and_block_comments_are_rejected() {
        for input in [r#"{"a"="open;}"#, "{/* open }"] {
            assert!(matches!(
                quote_launchctl_atoms(input.as_bytes()),
                Err(NativeError::LaunchdState(_))
            ));
        }
        assert!(quote_launchctl_atoms(b"{} // trailing line comment").is_ok());
    }
}
