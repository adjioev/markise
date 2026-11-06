//! A package as Markdown for those who write its blocks without opening the app, agents included:
//! what `markise-cli blocks docs` prints. Everything specific comes from the package's data, and the
//! same package gives the same bytes, so the result can sit in a repository with small diffs.

use std::fmt::Write;

use serde_json::Value;

use crate::form::ARGUMENT_KEY;
use crate::index::{BlockEntry, PackageIndex};
use crate::palette::Preset;
use crate::schema::{FieldKind, FieldSchema};
use crate::value::Localized;

/// What the first lines of the reference say of where it comes from.
#[derive(Clone, Copy, Debug)]
pub struct ReferenceHeader<'a> {
    /// The hash of the archive it was read from.
    pub sha256: &'a str,
    /// The command that makes it again.
    pub command: &'a str,
}

const GENERAL_RULES: &str = "## General rules

### Syntax

A block is an opener line, its content and a closer line:

```markdown
:::name arguments

Content.

:::
```

- An opener is at least three colons directly followed by the name, then optionally whitespace and
  arguments. `::: name`, `:::name:` and `:::name{x}` are text, not markers.
- A closer is colons and nothing else, at least as many as its opener has.
- A name is letters and digits of any script, `-` and `_`, starting with a letter or a digit.
- Markers count at the top level of a document only, not inside a quote or a list. In a code fence
  they are code.
- Put one blank line before and after each marker. Markise reads a marker without them and `check`
  does not report it, but the blank lines keep the source readable for people and other tools.

### Nesting

The outer block has the longer fence:

```markdown
::::outer

:::inner

Content.

:::

::::
```

A new opener ends every waiting opener with the same or a shorter fence, and those stay text. A
closer closes the outermost waiting opener whose fence is not longer than its own. A nested block is
content in no field of the block around it, so that block reports it unless its schema expects none.

### Fields

Each piece of a block's body goes to the first field, at or after the last one filled, that takes it.
A `text` field takes what no later field takes. What nothing takes is listed as `In no field`. A
setext heading is not a heading field.

### Ids

A heading, or an item of a list or a task list, can end in ` {#id}`: a stable name that readers do
not see. An answer of a `choices` field needs one, unique in its block: `- [ ] Text {#a1}`. Markise
gives a new answer `a` and a number above every number in the block; choose freely when writing by
hand, and do not change an id that is in use.

### What `markise-cli blocks check` reports

`markise-cli blocks check <file.md>...` reads the document's binding and the package from its archive,
with no question and nothing written. It reports, per reason, file, line, block and reason, in the
words of the editor's `⚠`:

| Kind | Headline | What it means |
|---|---|---|
| `not-filled` | Not filled in yet | a block with a view: a required field is empty or has too little in it, no answer is marked, or the argument is missing or not of its form |
| `structure` | Outside its schema | content in no field, arguments naming no variant, an answer with no id or a repeated one, too many items, too many marks; any reason of a block with no view; a block of over 64 KiB |
| `unknown-block` | Unknown block | the package has no block of this name |
| `no-form` | No form | the package says something about the block that a form cannot honour |
| `marker` | Marker | a marker that pairs with nothing, or a line of colons that is not a marker |

The exit code is 0 for clean, 1 for problems, 2 for a package or usage error. `--json` prints the
same as JSON. A file no binding applies to is noted and passes.
";

impl PackageIndex {
    /// The reference in `language`, as a document with that `lang` shows this package's text:
    /// each block in the palette's order, then those in no group by name, then the general rules.
    pub fn reference(&self, header: ReferenceHeader<'_>, language: &str) -> String {
        let id = self.id();
        let mut out = String::new();
        let _ = writeln!(out, "# Blocks of {} {}\n", id.library, id.version);
        let _ = writeln!(
            out,
            "Generated from the package, do not edit: library `{}`, version `{}`, sha256 `{}`, language `{}`.\n",
            id.library, id.version, header.sha256, language
        );
        let _ = writeln!(out, "To regenerate:\n\n```sh\n{}\n```\n", header.command);
        let mut listed = std::collections::BTreeSet::new();
        for group in self.groups() {
            let title = group.label.as_ref().map_or(group.id.as_str(), |l| l.get(language));
            let _ = writeln!(out, "## {}\n", one_line(title));
            for name in &group.blocks {
                listed.insert(name.as_str());
                self.write_block(&mut out, name, language);
            }
        }
        let rest: Vec<&str> = self.block_names().filter(|n| !listed.contains(n)).collect();
        if !rest.is_empty() {
            out.push_str("## In no group\n\n");
            for name in rest {
                self.write_block(&mut out, name, language);
            }
        }
        out.push_str(GENERAL_RULES);
        out
    }

    fn write_block(&self, out: &mut String, name: &str, language: &str) {
        let Some(entry) = self.block(name) else {
            return;
        };
        let text = |l: &Option<Localized>| {
            l.as_ref()
                .map(|l| l.get(language).to_string())
                .filter(|t| !t.is_empty())
        };
        let _ = match text(&entry.schema.label) {
            Some(label) => writeln!(out, "### `{name}`: {}\n", one_line(&label)),
            None => writeln!(out, "### `{name}`\n"),
        };
        if let Some(palette) = entry.palette {
            if let Some(description) = text(&palette.description) {
                let _ = writeln!(out, "{}\n", description.trim());
            }
            if let Some(when) = text(&palette.when) {
                let _ = writeln!(out, "**When to use:** {}\n", when.trim());
            }
        }
        write_arguments(out, &entry, name, language);
        match self.form_input(name, language) {
            Some(Err(refusal)) => {
                let _ = writeln!(
                    out,
                    "**The package says something a form cannot honour:** {refusal}. Blocks of this name stay cards.\n"
                );
            }
            Some(Ok(input)) => write_fields(out, &entry, &input.schema, language),
            None => {}
        }
        write_answer_options(out, self.presets_of(name));
        write_practice(out, &entry, language);
        write_presets(out, self.presets_of(name).collect(), language);
    }
}

fn write_arguments(out: &mut String, entry: &BlockEntry<'_>, name: &str, language: &str) {
    let schema = entry.schema;
    let argument = schema.raw.get("argument").and_then(Value::as_object);
    let variants: Vec<&str> = schema.variants().collect();
    if !variants.is_empty() {
        out.push_str("**Arguments** name a variant; anything else is outside the schema:\n\n");
        let _ = writeln!(out, "- no arguments, `:::{name}`{}", variant_words(entry, "", language));
        for v in variants.iter().filter(|v| !v.is_empty()) {
            let _ = writeln!(out, "- `:::{name} {v}`{}", variant_words(entry, v, language));
        }
        out.push('\n');
        if argument.is_some() {
            out.push_str("The block also has a form field for them; the variants are their check.\n\n");
        }
        return;
    }
    let Some(argument) = argument else {
        out.push_str("**Arguments:** none declared.\n\n");
        return;
    };
    let label = localized_value(argument.get("label"), language);
    let what = (argument.get("name").and_then(Value::as_str))
        .map(|n| format!("`{n}`"))
        .into_iter()
        .chain(label.map(one_line))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = write!(out, "**Argument:**");
    if !what.is_empty() {
        let _ = write!(out, " {what};");
    }
    out.push_str(" required, a missing one reads as not filled in");
    if let Some(pattern) = argument.get("pattern").and_then(Value::as_str) {
        let _ = write!(out, "; searched for the pattern {}", code(pattern));
    }
    out.push_str(".\n\n");
    if let Some(help) = localized_value(argument.get("help"), language) {
        let _ = writeln!(out, "{}\n", help.trim());
    }
}

fn variant_words(entry: &BlockEntry<'_>, variant: &str, language: &str) -> String {
    (entry.schema.variant_label(variant, language))
        .filter(|l| !l.is_empty())
        .map_or(String::new(), |l| format!(": {}", one_line(l)))
}

fn write_fields(out: &mut String, entry: &BlockEntry<'_>, form: &Value, language: &str) {
    if entry.schema.fields.is_empty() {
        out.push_str("**Fields:** none.\n\n");
        return;
    }
    out.push_str("| Field | Label | In the Markdown | Rules |\n|---|---|---|---|\n");
    let required: Vec<&str> = (form.get("required").and_then(Value::as_array).into_iter().flatten())
        .filter_map(Value::as_str)
        .filter(|k| *k != ARGUMENT_KEY)
        .collect();
    for f in &entry.schema.fields {
        let label = localized_value(f.raw.get("label"), language).map_or(String::new(), one_line);
        let rules = field_rules(f, required.contains(&f.name.as_str()), language).join("; ");
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} |",
            f.name,
            cell(&label),
            markdown_of(f),
            cell(&rules)
        );
    }
    out.push('\n');
}

fn markdown_of(f: &FieldSchema) -> String {
    match f.kind {
        FieldKind::Heading => match f.raw.get("level").and_then(Value::as_u64) {
            Some(level) => format!("heading: one ATX heading of level {level}"),
            None => "heading: one ATX heading".to_string(),
        },
        FieldKind::Text => "text: paragraphs, and what no later field takes".to_string(),
        FieldKind::Quote => "quote: one block quote".to_string(),
        FieldKind::List => "list: one list".to_string(),
        FieldKind::Choices => "choices: one task list".to_string(),
        FieldKind::Image => "image: paragraphs holding only images, one per line".to_string(),
    }
}

fn field_rules(f: &FieldSchema, required: bool, language: &str) -> Vec<String> {
    let mut rules = vec![if required { "required" } else { "optional" }.to_string()];
    let number = |keys: [&str; 2]| keys.iter().find_map(|k| f.raw.get(*k).and_then(Value::as_u64));
    match f.kind {
        FieldKind::Heading | FieldKind::Text | FieldKind::Quote => {}
        FieldKind::List | FieldKind::Choices | FieldKind::Image => {
            if let Some(n) = number(["minItems", "min"]).filter(|n| *n > 0) {
                rules.push(format!("at least {n}"));
            }
            if let Some(n) = number(["maxItems", "max"]) {
                rules.push(format!("at most {n}"));
            }
        }
    }
    if f.kind == FieldKind::List {
        rules.push("no empty item".to_string());
    }
    if f.kind == FieldKind::Image {
        rules.push("each with a path".to_string());
    }
    if f.kind == FieldKind::Choices {
        rules.extend(correct_rule(f));
        rules.push("no empty answer".to_string());
        rules.push("every answer has a `{#id}`, unique in the block".to_string());
    }
    if let Some(help) = localized_value(f.raw.get("help"), language) {
        rules.push(format!("help: {}", one_line(help)));
    }
    rules
}

fn correct_words(rule: &str) -> String {
    match rule {
        "exactly-one" => "mark exactly one".to_string(),
        "at-least-one" => "mark at least one".to_string(),
        other => format!("`correct` is `{other}`"),
    }
}

fn correct_rule(f: &FieldSchema) -> Vec<String> {
    match f.raw.get("correct") {
        Some(Value::String(rule)) => vec![correct_words(rule)],
        Some(Value::Object(per_variant)) => (per_variant.iter())
            .filter_map(|(variant, rule)| {
                let rule = correct_words(rule.as_str()?);
                Some(if variant.is_empty() {
                    format!("with no variant, {rule}")
                } else {
                    format!("in `{variant}`, {rule}")
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A preset that makes a field's answers a choice among known ones: anything else is outside the schema.
fn write_answer_options<'a>(out: &mut String, presets: impl Iterator<Item = &'a Preset>) {
    for p in presets {
        if p.extra.get("widget").and_then(Value::as_str) != Some("icon-choices") {
            continue;
        }
        let Some(answers) = p.extra.get("answers").and_then(Value::as_array) else {
            continue;
        };
        let field = p.extra.get("field").and_then(Value::as_str);
        let variant = p.extra.get("variant").and_then(Value::as_str);
        let place = match variant {
            Some("") => " with no variant".to_string(),
            Some(v) => format!(" in `{v}`"),
            None => String::new(),
        };
        let what = field.map_or("answers".to_string(), |f| format!("answers of `{f}`"));
        let _ = writeln!(
            out,
            "**The {what}{place}** are one of these, with this very text; those marked as exclusive go alone:\n"
        );
        for a in answers {
            let Some(label) = a.get("label").and_then(Value::as_str) else {
                continue;
            };
            let exclusive = a.get("exclusive") == Some(&Value::Bool(true));
            let _ = writeln!(out, "- {}{}", code(label), if exclusive { " (exclusive)" } else { "" });
        }
        out.push('\n');
    }
}

fn write_practice(out: &mut String, entry: &BlockEntry<'_>, language: &str) {
    let Some(practice) = entry.view.and_then(|v| v.practice.as_ref()) else {
        out.push_str("**Practice:** none, nothing of it is hidden in Read.\n\n");
        return;
    };
    let names = |fields: &[String]| {
        (fields.iter())
            .map(|field| {
                let found = entry.schema.fields.iter().find(|f| f.name == *field);
                let label = found.and_then(|f| localized_value(f.raw.get("label"), language));
                match label {
                    Some(label) => format!("`{field}` ({})", one_line(label)),
                    None => format!("`{field}`"),
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let has_choices = entry.schema.fields.iter().any(|f| f.kind == FieldKind::Choices);
    if has_choices {
        out.push_str("**Practice:** the marks of the answers are hidden in Read until the reader checks, and the block counts in the score.");
        if !practice.reveal.is_empty() {
            let _ = write!(out, " Hidden with them: {}.", names(&practice.reveal));
        }
    } else {
        let _ = write!(
            out,
            "**Practice:** hidden in Read until the reader asks, behind Show answer: {}. It does not count in the score.",
            names(&practice.reveal)
        );
    }
    if practice.default {
        out.push_str(" A document holding this block opens in Read.");
    }
    out.push_str("\n\n");
}

fn write_presets(out: &mut String, presets: Vec<&Preset>, language: &str) {
    if presets.is_empty() {
        out.push_str("**Example:** the package has no preset, so the block cannot be inserted from the library.\n\n");
        return;
    }
    for p in presets {
        let label = p.label.as_ref().map(|l| l.get(language)).filter(|l| !l.is_empty());
        let _ = writeln!(out, "**Example:** {}\n", one_line(label.unwrap_or(&p.id)));
        let fence = "`".repeat(longest_backticks(&p.markdown).max(2) + 1);
        let _ = writeln!(out, "{fence}markdown\n{}\n{fence}\n", p.markdown.trim_end());
    }
}

fn longest_backticks(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

/// Text for the user in a schema's raw JSON, as [`Localized`] picks it.
fn localized_value<'a>(v: Option<&'a Value>, language: &str) -> Option<&'a str> {
    crate::form::text_of(v, language).filter(|t| !t.is_empty())
}

/// `text` as inline code, whatever backticks it holds.
fn code(text: &str) -> String {
    let fence = "`".repeat(longest_backticks(text) + 1);
    if text.starts_with('`') || text.ends_with('`') {
        format!("{fence} {text} {fence}")
    } else {
        format!("{fence}{text}{fence}")
    }
}

/// The text on one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text in a cell of a table.
fn cell(text: &str) -> String {
    one_line(text).replace('|', "\\|")
}
