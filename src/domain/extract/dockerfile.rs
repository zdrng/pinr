//! Dockerfiles and Containerfiles: the `# syntax=` directive, `FROM` and the
//! `--from=` of `COPY`, `ADD` and `RUN --mount`.

use super::{Extractor, image};
use crate::domain::model::Found;

pub struct Dockerfile;

impl Extractor for Dockerfile {
    fn extract(&self, src: &str) -> Vec<Found> {
        let lines = lines(src);
        let mut found = Vec::new();
        let mut stages = Vec::new();
        let mut escape = '\\';
        // Parser directives are only valid before the first other line.
        let mut in_header = true;
        let mut i = 0;
        while i < lines.len() {
            let (offset, line) = lines[i];
            if in_header {
                let known = directive(line).filter(|(key, ..)| {
                    ["syntax", "escape", "check"]
                        .iter()
                        .any(|k| key.eq_ignore_ascii_case(k))
                });
                if let Some((key, value, at)) = known {
                    if key.eq_ignore_ascii_case("syntax") {
                        found.push(image(offset + at, value));
                    } else if key.eq_ignore_ascii_case("escape") {
                        escape = value.chars().next().unwrap_or(escape);
                    }
                    i += 1;
                    continue;
                }
                // Anything else, even an unknown directive, ends the header.
                in_header = false;
            }
            if is_blank_or_comment(line) {
                i += 1;
                continue;
            }

            // Join an instruction's continuation lines, skipping comments in between.
            let mut words = Vec::new();
            loop {
                let (offset, line) = lines[i];
                i += 1;
                let line = line.trim_end();
                let (body, more) = match line.strip_suffix(escape) {
                    Some(body) => (body, true),
                    None => (line, false),
                };
                words.extend(split_words(body, offset));
                while more && lines.get(i).is_some_and(|&(_, l)| is_blank_or_comment(l)) {
                    i += 1;
                }
                if !more || i == lines.len() {
                    break;
                }
            }

            // Heredoc bodies follow the instruction and aren't instructions themselves.
            for &(_, word) in &words {
                let Some((delimiter, strip_tabs)) = heredoc(word) else {
                    continue;
                };
                while let Some(&(_, line)) = lines.get(i) {
                    i += 1;
                    let line = if strip_tabs {
                        line.trim_start_matches('\t')
                    } else {
                        line
                    };
                    if line == delimiter {
                        break;
                    }
                }
            }

            instruction(&words, &mut stages, &mut found);
        }
        found
    }
}

/// Look for images in one instruction, given as words with their offsets in the file.
/// Remembers the stage names of `FROM … AS name`.
fn instruction<'a>(words: &[(usize, &'a str)], stages: &mut Vec<&'a str>, found: &mut Vec<Found>) {
    let Some((&(_, cmd), args)) = words.split_first() else {
        return;
    };
    let flags = args.iter().take_while(|(_, w)| w.starts_with("--"));
    match cmd.to_ascii_uppercase().as_str() {
        "FROM" => {
            let mut rest = args.iter().skip_while(|(_, w)| w.starts_with("--"));
            let Some(&(at, name)) = rest.next() else {
                return;
            };
            if !is_internal(name, stages) {
                found.push(image(at, name));
            }
            if let (Some((_, kw)), Some(&(_, stage))) = (rest.next(), rest.next())
                && kw.eq_ignore_ascii_case("as")
            {
                stages.push(stage);
            }
        }
        "COPY" | "ADD" => {
            for &(at, flag) in flags {
                if let Some(from) = flag.strip_prefix("--from=")
                    && !is_internal(from, stages)
                {
                    found.push(image(at + "--from=".len(), from));
                }
            }
        }
        "RUN" => {
            for &(at, flag) in flags {
                let Some(mount) = flag.strip_prefix("--mount=") else {
                    continue;
                };
                let mut at = at + "--mount=".len();
                for option in mount.split(',') {
                    if let Some(from) = option.strip_prefix("from=")
                        && !is_internal(from, stages)
                    {
                        found.push(image(at + "from=".len(), from));
                    }
                    at += option.len() + 1;
                }
            }
        }
        _ => {}
    }
}

/// `scratch`, an earlier stage's name or a numeric stage index.
fn is_internal(name: &str, stages: &[&str]) -> bool {
    name.eq_ignore_ascii_case("scratch")
        || name.bytes().all(|b| b.is_ascii_digit())
        || stages.iter().any(|s| s.eq_ignore_ascii_case(name))
}

/// A parser directive like `# syntax=docker/dockerfile:1`: key, value and the
/// value's offset in the line.
fn directive(line: &str) -> Option<(&str, &str, usize)> {
    let (key, value) = line.trim_start().strip_prefix('#')?.split_once('=')?;
    let key = key.trim();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    let value = value.trim_start();
    Some((key, value.trim_end(), line.len() - value.len()))
}

/// The delimiter of a heredoc word like `<<EOF`, `<<-EOF` or `<<"EOF"`, and
/// whether leading tabs are stripped (`<<-`).
fn heredoc(word: &str) -> Option<(&str, bool)> {
    let rest = word.strip_prefix("<<")?;
    let (rest, strip_tabs) = match rest.strip_prefix('-') {
        Some(rest) => (rest, true),
        None => (rest, false),
    };
    let rest = rest.trim_start_matches(['"', '\'']);
    let len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    (len > 0).then(|| (&rest[..len], strip_tabs))
}

fn is_blank_or_comment(line: &str) -> bool {
    let line = line.trim_start();
    line.is_empty() || line.starts_with('#')
}

/// Whitespace-separated words of a line with their offsets in the file.
fn split_words(line: &str, offset: usize) -> Vec<(usize, &str)> {
    let mut words = Vec::new();
    let mut rest = line.trim_start();
    while !rest.is_empty() {
        let len = rest.find(char::is_whitespace).unwrap_or(rest.len());
        words.push((offset + line.len() - rest.len(), &rest[..len]));
        rest = rest[len..].trim_start();
    }
    words
}

/// Lines without their line ending (`\n` or `\r\n`), with their offsets in the file.
fn lines(src: &str) -> Vec<(usize, &str)> {
    let mut offset = 0;
    src.split_inclusive('\n')
        .map(|line| {
            let start = offset;
            offset += line.len();
            (start, line.trim_end_matches(['\n', '\r']))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{Kind, Reference};
    use crate::domain::refs::parse_image;

    fn extract(src: &str) -> Vec<(&str, Kind)> {
        Dockerfile
            .extract(src)
            .into_iter()
            .map(|f| (&src[f.span], f.kind))
            .collect()
    }

    fn img(s: &str) -> Kind {
        Kind::Ref(Reference::Image(parse_image(s).unwrap()))
    }

    fn warn(s: &str) -> Kind {
        Kind::Warning(parse_image(s).unwrap_err())
    }

    #[test]
    fn multi_stage_build() {
        let src = "\
FROM --platform=$BUILDPLATFORM golang:1.22 AS Build
WORKDIR /src
RUN --mount=type=cache,target=/go --mount=type=bind,from=build,target=/x go build
RUN --mount=type=bind,from=alpine:3.20,target=/a true
from build as test
COPY --from=Build /out/app /app
COPY --from=0 /out/app /app
COPY --link --from=nginx:1.27 /etc/nginx /etc/nginx
ADD --from=ghcr.io/org/tools:1@sha256:abc /bin/tool /bin/
FROM scratch
COPY --from=test /out/app /app
";
        assert_eq!(
            extract(src),
            vec![
                ("golang:1.22", img("golang:1.22")),
                ("alpine:3.20", img("alpine:3.20")),
                ("nginx:1.27", img("nginx:1.27")),
                (
                    "ghcr.io/org/tools:1@sha256:abc",
                    img("ghcr.io/org/tools:1@sha256:abc")
                ),
            ]
        );
    }

    #[test]
    fn variables_are_warned() {
        let src = "ARG NODE=20\nFROM node:${NODE}-alpine\nFROM $BASE\n";
        assert_eq!(
            extract(src),
            vec![
                ("node:${NODE}-alpine", warn("node:${NODE}-alpine")),
                ("$BASE", warn("$BASE")),
            ]
        );
    }

    #[test]
    fn syntax_directive_only_in_header() {
        let src = "# syntax = docker/dockerfile:1.7\n# check=error=true\n\n# syntax=evil:1\nFROM node:20\n";
        assert_eq!(
            extract(src),
            vec![
                ("docker/dockerfile:1.7", img("docker/dockerfile:1.7")),
                ("node:20", img("node:20")),
            ]
        );

        let src = "# unknown=1\n# syntax=evil:1\nFROM node:20\n";
        assert_eq!(extract(src), vec![("node:20", img("node:20"))]);
    }

    #[test]
    fn line_continuations_and_crlf() {
        let src = "# comment\r\nfrom \\\r\n  # inside\r\n  --platform=linux/amd64 \\\r\n  node:20-alpine \\\r\n  AS base\r\nCOPY --from=base / /\r\nRUN echo \\\r\nFROM evil\r\n";
        assert_eq!(
            extract(src),
            vec![("node:20-alpine", img("node:20-alpine"))]
        );
    }

    #[test]
    fn escape_directive() {
        let src = "# escape=`\nFROM `\n  node:20\nRUN dir c:\\\nFROM evil\n";
        assert_eq!(
            extract(src),
            vec![("node:20", img("node:20")), ("evil", img("evil"))]
        );
    }

    #[test]
    fn heredoc_bodies_are_skipped() {
        let src = "\
FROM debian:12
RUN <<EOF
FROM evil
EOF
COPY <<-\"END\" /etc/conf
\tFROM evil
\tEND
RUN cat <<'A' > /a && cat <<B > /b
FROM evil
A
FROM evil
B
FROM alpine:3
";
        assert_eq!(
            extract(src),
            vec![
                ("debian:12", img("debian:12")),
                ("alpine:3", img("alpine:3"))
            ]
        );
    }
}
