use std::collections::{BTreeMap, HashMap};
use std::io::Write;

use crate::export::{Error, FileStats, Options, Written, finished_stats, write_file};
use crate::image_data::text_at;
use crate::plan;
use crate::render::{MARKER, View};

const USERS_SHOWN: usize = 20;
const INDEX_STRING_WIDTH: usize = 120;

pub struct Inputs<'a> {
    pub view: &'a View<'a>,
    pub options: &'a Options,
    /// Totals of the files written or kept in this run, by group.
    pub stats: &'a HashMap<usize, FileStats>,
    pub headers: &'a [Written],
    /// Whether `--only` limited the export: the long listings are then left as they were.
    pub partial: bool,
}

pub fn write(inputs: &Inputs<'_>) -> Result<(), Error> {
    let out = &inputs.options.out;
    write_main(inputs)?;
    if inputs.partial {
        return Ok(());
    }
    let view = inputs.view;
    let facts = view.facts;
    let by_name = {
        let mut order: Vec<u32> = (0..facts.entries.len() as u32).collect();
        order.sort_by_cached_key(|&i| (view.name(i as usize).to_ascii_lowercase(), i));
        order
    };

    write_file(&out.join("index/functions.md"), |w| {
        writeln!(w, "{MARKER}\n\n# Functions by name\n")?;
        for &i in &by_name {
            let i = i as usize;
            writeln!(
                w,
                "- {}  0x{:08x}  {}.c",
                view.name(i),
                facts.entries[i].rva,
                view.path_of(i)
            )?;
        }
        Ok(())
    })?;

    write_file(&out.join("index/imports.md"), |w| {
        writeln!(w, "{MARKER}\n\n# Imports by caller\n")?;
        for &i in &by_name {
            let i = i as usize;
            if facts.imports[i].is_empty() {
                continue;
            }
            let names: Vec<String> = facts.imports[i]
                .iter()
                .map(|&n| {
                    let import = view.imports.get(n as usize);
                    format!("{}!{}", import.dll, import.name)
                })
                .collect();
            writeln!(
                w,
                "- {}  0x{:08x}: {}",
                view.name(i),
                facts.entries[i].rva,
                names.join(", ")
            )?;
        }
        Ok(())
    })?;

    let mut users: HashMap<u32, Vec<u32>> = HashMap::new();
    for (i, strings) in facts.strings.iter().enumerate() {
        for &rva in strings {
            users.entry(rva).or_default().push(i as u32);
        }
    }
    let mut strings: Vec<(String, u32)> = users
        .keys()
        .filter_map(|&rva| text_at(view.image, rva).map(|t| (t, rva)))
        .collect();
    strings.sort();
    write_file(&out.join("index/strings.md"), |w| {
        writeln!(w, "{MARKER}\n\n# Strings by user\n")?;
        for (text, rva) in &strings {
            let mut shown: String = text.chars().take(INDEX_STRING_WIDTH).collect();
            if text.chars().count() > INDEX_STRING_WIDTH {
                shown.push_str("...");
            }
            let list = &users[rva];
            let names: Vec<&str> = list
                .iter()
                .take(USERS_SHOWN)
                .map(|&i| view.name(i as usize))
                .collect();
            let more = list.len().saturating_sub(USERS_SHOWN);
            let tail = if more > 0 {
                format!(", ... (+{more} more)")
            } else {
                String::new()
            };
            writeln!(w, "- {shown:?}  0x{rva:08x}: {}{tail}", names.join(", "))?;
        }
        Ok(())
    })?;

    let mut callers: Vec<Vec<u32>> = vec![Vec::new(); view.imports.len()];
    for (i, list) in facts.imports.iter().enumerate() {
        for &n in list {
            callers[n as usize].push(i as u32);
        }
    }
    let mut dlls: BTreeMap<String, (String, Vec<usize>)> = BTreeMap::new();
    for (n, import) in view.imports.iter() {
        dlls.entry(import.dll.to_ascii_lowercase())
            .or_insert_with(|| (import.dll.clone(), Vec::new()))
            .1
            .push(n);
    }
    for (lower, (dll, mut list)) in dlls {
        list.sort_by_key(|&n| view.imports.get(n).name.to_ascii_lowercase());
        let file = plan::component(&lower).unwrap_or_else(|| "unnamed".into());
        write_file(&out.join(format!("imports/{file}.md")), |w| {
            writeln!(w, "{MARKER}\n\n# {dll}\n")?;
            for &n in &list {
                let users = &callers[n];
                writeln!(w, "## {}\n", view.imports.get(n).name)?;
                if users.is_empty() {
                    writeln!(w, "No direct callers found.\n")?;
                    continue;
                }
                for &i in users {
                    let i = i as usize;
                    writeln!(
                        w,
                        "- {}  0x{:08x}  {}.c",
                        view.name(i),
                        facts.entries[i].rva,
                        view.path_of(i)
                    )?;
                }
                writeln!(w)?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn write_main(inputs: &Inputs<'_>) -> Result<(), Error> {
    let view = inputs.view;
    let plan = view.plan;
    let out = &inputs.options.out;
    let mut text = String::new();
    text.push_str("# Source export\n\n");
    text.push_str(&format!(
        "{} functions in {} files, written from the executable's code.\n\n",
        view.facts.entries.len(),
        plan.groups.len()
    ));
    text.push_str("Listings:\n\n");
    text.push_str("- [functions by name](index/functions.md)\n");
    text.push_str("- [imports by caller](index/imports.md)\n");
    text.push_str("- [strings by user](index/strings.md)\n");
    text.push_str("- `imports/<dll>.md`: each imported function and every caller\n");
    if inputs.partial {
        text.push_str(
            "\nThis run exported a subset with `--only`: the listings above were not rewritten.\n",
        );
    }
    text.push_str(
        "\n## Files\n\n| file | kind | functions | stubbed | failed |\n|---|---|---|---|---|\n",
    );
    for (g, group) in plan.groups.iter().enumerate() {
        let file = format!("{}.c", group.path);
        let stats = inputs
            .stats
            .get(&g)
            .copied()
            .or_else(|| finished_stats(&out.join(&file)));
        match stats {
            Some(s) => text.push_str(&format!(
                "| [{file}]({file}) | {} | {} | {} | {} |\n",
                group.kind.label(),
                s.functions,
                s.stubbed,
                s.failed
            )),
            None => text.push_str(&format!(
                "| {file} | {} | {} | not exported | |\n",
                group.kind.label(),
                group.members.len()
            )),
        }
    }
    if !inputs.headers.is_empty() {
        text.push_str("\n## Class headers\n\n");
        for header in inputs.headers {
            text.push_str(&format!("- [{0}]({0})\n", header.path));
        }
    }
    crate::export::write_text(&out.join("index.md"), &text)
}
