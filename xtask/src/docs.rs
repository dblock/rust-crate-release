use crate::Result;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::fs;
use std::path::Path;
use yaml_rust2::{Yaml, YamlLoader};

fn title(text: &str) -> std::result::Result<(), String> {
    let text = without_expressions(text);
    let words: Vec<_> = text
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric() && c != '.'))
        .filter(|word| !word.is_empty())
        .collect();
    for (index, word) in words.iter().enumerate() {
        if matches!(*word, "rust-crate-release" | "crates.io" | "npm" | "rcr") {
            continue;
        }
        let small = matches!(
            word.to_ascii_lowercase().as_str(),
            "a" | "an"
                | "and"
                | "as"
                | "at"
                | "but"
                | "by"
                | "for"
                | "in"
                | "nor"
                | "of"
                | "on"
                | "or"
                | "per"
                | "the"
                | "to"
                | "via"
                | "with"
        );
        if small && index > 0 && index + 1 < words.len() {
            if word.chars().any(char::is_uppercase) {
                return Err(format!("use lowercase for interior title word {word:?}"));
            }
        } else if word.chars().next().is_some_and(char::is_lowercase) {
            return Err(format!("capitalize title word {word:?}"));
        }
    }
    Ok(())
}

fn without_expressions(text: &str) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${{") {
        result.push_str(&rest[..start]);
        let Some(end) = rest[start + 3..].find("}}") else {
            result.push_str(&rest[start..]);
            return result;
        };
        result.push_str("Expression");
        rest = &rest[start + 3 + end + 2..];
    }
    result.push_str(rest);
    result
}

fn markdown(source: &str) -> std::result::Result<(), String> {
    let mut heading = None;
    for event in Parser::new_ext(source, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::Heading { .. }) => heading = Some(String::new()),
            Event::End(TagEnd::Heading(_)) => title(&heading.take().unwrap())?,
            Event::Text(text) => {
                if let Some(heading) = heading.as_mut() {
                    heading.push_str(&text);
                }
            }
            Event::Code(_) => {
                if let Some(heading) = heading.as_mut() {
                    heading.push_str(" Identifier ");
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                return Err("keep each Markdown paragraph on one source line".into());
            }
            _ => {}
        }
    }
    Ok(())
}

fn display_name(node: &Yaml) -> std::result::Result<(), String> {
    match &node["name"] {
        Yaml::BadValue => Ok(()),
        Yaml::String(name) => title(name),
        _ => Err("workflow display names must be strings".into()),
    }
}

fn workflow(source: &str) -> std::result::Result<(), String> {
    let documents = YamlLoader::load_from_str(source).map_err(|error| error.to_string())?;
    if documents.len() != 1 || documents[0].as_hash().is_none() {
        return Err("expected one workflow YAML mapping".into());
    }
    let document = &documents[0];
    display_name(document)?;
    if let Some(jobs) = document["jobs"].as_hash() {
        for job in jobs.values() {
            display_name(job)?;
            if let Some(steps) = job["steps"].as_vec() {
                for step in steps {
                    display_name(step)?;
                }
            }
        }
    }
    Ok(())
}

pub fn check(root: &Path) -> Result {
    let mut errors = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "md") {
            markdown(&fs::read_to_string(&path)?)
                .unwrap_or_else(|error| errors.push(format!("{}: {error}", path.display())));
        }
    }
    for entry in fs::read_dir(root.join(".github/workflows"))? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "yml" || extension == "yaml")
        {
            workflow(&fs::read_to_string(&path)?)
                .unwrap_or_else(|error| errors.push(format!("{}: {error}", path.display())));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_checks_titles_without_mistaking_code_for_prose() {
        assert!(markdown("# Working on rust-crate-release\n\n## Setup and Validation\n\n## `rcr prepare` and GitHub Actions\n\n```md\n# not a heading\n```\n").is_ok());
        assert!(markdown("## Setup and validation\n").is_err());
        assert!(markdown("## Setup And Validation\n").is_err());
        assert!(markdown("Setup and validation\n---\n").is_err());
    }

    #[test]
    fn markdown_rejects_wrapped_prose_but_allows_independent_blocks() {
        assert!(markdown("A paragraph\ncontinued here.\n").is_err());
        assert!(markdown("A paragraph  \ncontinued here.\n").is_err());
        assert!(markdown("- An item\n  continued here.\n").is_err());
        assert!(markdown("A paragraph.\n\nAnother paragraph.\n\n- One item.\n- Another item.\n\n```rs\nlet a = 1;\nlet b = 2;\n```\n").is_ok());
        assert!(
            markdown("| Area | Requirement |\n| --- | --- |\n| Cargo | Exact Bytes |\n").is_ok()
        );
    }

    #[test]
    fn yaml_checks_only_workflow_job_and_step_display_names() {
        assert!(workflow("name: Test\non: push\njobs:\n  unit_tests:\n    name: Rust ${{ matrix.entry.rust }} on ${{ matrix.entry.os }}\n    runs-on: ubuntu-latest\n    steps:\n      - name: Run Tests\n        run: cargo test\n      - uses: example/action@v1\n        with:\n          name: lowercase artifact name\n").is_ok());
        for source in [
            "name: run tests\njobs: {}\n",
            "jobs:\n  test:\n    name: run Tests\n",
            "jobs:\n  test:\n    steps:\n      - name: Run tests\n",
            "name: 42\n",
        ] {
            assert!(workflow(source).is_err(), "{source}");
        }
    }

    #[test]
    fn yaml_rejects_invalid_syntax_and_multiple_documents() {
        assert!(workflow("name: [\n").is_err());
        assert!(workflow("name: Test\n---\nname: Lint\n").is_err());
        assert!(workflow("- Not a mapping\n").is_err());
        assert!(workflow("jobs:\n  build:\n    steps:\n      - run: echo ok\n").is_ok());
    }

    #[test]
    fn titles_preserve_proper_names_acronyms_and_expressions() {
        for name in [
            "Build the Cargo Archive",
            "Run GitHub Actions",
            "Upload LCOV to Coveralls",
            "${{ matrix.entry.name }}",
            "Rust ${{ matrix.entry.rust }} on Windows",
        ] {
            assert!(title(name).is_ok(), "{name}");
        }
        assert_eq!(
            without_expressions("Rust ${{ matrix.entry.rust }} Tests"),
            "Rust Expression Tests"
        );
        assert_eq!(
            without_expressions("Rust ${{ unfinished"),
            "Rust ${{ unfinished"
        );
    }
}
