//! Pairing programs without root signatures: the game names a program `<Stage>_<Name>_<hash>_Wrapped`, and
//! programs of different stages that share the hash show the same header user SGPR count and the same use of
//! each parameter SGPR, so the hash stands for the root signature they were compiled against.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::program::Program;
use crate::summary::group_name;

/// The eight-digit hash a program name ends in, before `_Wrapped`.
pub fn technique(name: &str) -> Option<&str> {
    let name = name.strip_suffix("_Wrapped").unwrap_or(name);
    let (_, hash) = name.rsplit_once('_')?;
    (hash.len() == 8 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hash)
}

/// How the same-hash programs of two stages agree when the second stage's SGPR numbers are `shift` higher.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct ShiftCount {
    pub shift: i32,
    /// Entry SGPRs both programs read, used the same way.
    pub agree: usize,
    pub differ: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StagePair {
    pub from: String,
    pub to: String,
    /// Hashes that have a program of each stage (two or more of the stage when `from` equals `to`).
    pub hashes: usize,
    /// Of those, hashes where each stage has one header user SGPR count and the two are equal.
    pub equal_header: usize,
    /// Of those, hashes where each stage has one count and the two differ.
    pub unequal_header: usize,
    pub shifts: Vec<ShiftCount>,
}

impl StagePair {
    /// The shift with the most agreement beyond disagreement, the smaller one on a tie.
    pub fn best(&self) -> Option<&ShiftCount> {
        self.shifts.iter().max_by_key(|c| {
            (
                c.agree as i64 - c.differ as i64,
                std::cmp::Reverse(c.shift.abs()),
            )
        })
    }

    pub fn at(&self, shift: i32) -> Option<&ShiftCount> {
        self.shifts.iter().find(|c| c.shift == shift)
    }
}

/// Programs of one stage grouped by hash.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct StageHashes {
    pub stage: String,
    pub programs: usize,
    pub hashes: usize,
    /// Hashes whose programs of this stage all have the same header user SGPR count.
    pub single_header: usize,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Techniques {
    pub with_hash: usize,
    pub without_hash: usize,
    pub distinct_hashes: usize,
    pub stages: Vec<StageHashes>,
    pub pairs: Vec<StagePair>,
    /// Per stage, the shift-0 comparison between programs picked without regard to hash: agreeing and
    /// differing reads. This is the rate unrelated programs disagree at.
    pub control: Vec<(String, usize, usize)>,
}

const SHIFTS: std::ops::RangeInclusive<i32> = -8..=16;

const PAIRS: &[(&str, &str)] = &[
    ("ps", "vs"),
    ("ps", "ms"),
    ("ps", "gs"),
    ("ps", "vs-first-half"),
    ("vs", "vs-first-half"),
    ("ps", "ps"),
    ("vs", "vs"),
    ("ms", "ms"),
    ("cs", "cs"),
];

type Groups<'a> = BTreeMap<&'a str, BTreeMap<String, Vec<&'a Program>>>;

fn compare(a: &Program, b: &Program, count: &mut ShiftCount) {
    for r in a.reads.iter() {
        let rb = r as i32 + count.shift;
        if !(0..32).contains(&rb) || !b.reads.contains(rb as u32) {
            continue;
        }
        if a.use_of(r) == b.use_of(rb as u32) {
            count.agree += 1;
        } else {
            count.differ += 1;
        }
    }
}

/// The header user SGPR count the programs share, if they share one.
fn header_of(list: &[&Program]) -> Option<u32> {
    let first = list.first()?.header_user_sgprs;
    list.iter()
        .all(|p| p.header_user_sgprs == first)
        .then_some(first)
}

fn pair_of(groups: &Groups<'_>, from: &str, to: &str) -> StagePair {
    let mut pair = StagePair {
        from: from.to_string(),
        to: to.to_string(),
        hashes: 0,
        equal_header: 0,
        unequal_header: 0,
        shifts: SHIFTS
            .map(|shift| ShiftCount {
                shift,
                agree: 0,
                differ: 0,
            })
            .collect(),
    };
    for by_stage in groups.values() {
        let (Some(a), Some(b)) = (by_stage.get(from), by_stage.get(to)) else {
            continue;
        };
        if from == to && a.len() < 2 {
            continue;
        }
        pair.hashes += 1;
        if from != to
            && let (Some(ha), Some(hb)) = (header_of(a), header_of(b))
        {
            if ha == hb {
                pair.equal_header += 1;
            } else {
                pair.unequal_header += 1;
            }
        }
        for (i, pa) in a.iter().enumerate() {
            for (j, pb) in b.iter().enumerate() {
                if from == to && j <= i {
                    continue;
                }
                for count in &mut pair.shifts {
                    compare(pa, pb, count);
                }
            }
        }
    }
    pair
}

impl Techniques {
    pub fn of(programs: &[Program]) -> Techniques {
        let mut out = Techniques::default();
        let mut groups: Groups<'_> = BTreeMap::new();
        for program in programs {
            match technique(&program.name) {
                Some(hash) => {
                    out.with_hash += 1;
                    groups
                        .entry(hash)
                        .or_default()
                        .entry(group_name(program))
                        .or_default()
                        .push(program);
                }
                None => out.without_hash += 1,
            }
        }
        out.distinct_hashes = groups.len();
        let mut stages: BTreeMap<&str, StageHashes> = BTreeMap::new();
        for by_stage in groups.values() {
            for (stage, list) in by_stage {
                let entry = stages.entry(stage).or_default();
                entry.stage = stage.clone();
                entry.programs += list.len();
                entry.hashes += 1;
                entry.single_header += usize::from(header_of(list).is_some());
            }
        }
        out.stages = stages.into_values().collect();
        out.pairs = PAIRS
            .iter()
            .map(|&(from, to)| pair_of(&groups, from, to))
            .collect();
        for stage in ["ps", "vs", "cs", "ms"] {
            let list: Vec<&Program> = programs
                .iter()
                .filter(|p| !p.first_half && p.stage == stage)
                .collect();
            let mut count = ShiftCount {
                shift: 0,
                agree: 0,
                differ: 0,
            };
            for (i, a) in list.iter().enumerate().take(3000) {
                let b = list[(i * 7919 + 13) % list.len()];
                if !std::ptr::eq(*a, b) {
                    compare(a, b, &mut count);
                }
            }
            out.control
                .push((stage.to_string(), count.agree, count.differ));
        }
        out
    }

    pub fn pair(&self, from: &str, to: &str) -> Option<&StagePair> {
        self.pairs.iter().find(|p| p.from == from && p.to == to)
    }

    pub fn write(&self, out: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(
            out,
            "== programs sharing a name hash ==\n{} of {} programs end in an eight-digit hash, {} distinct",
            self.with_hash,
            self.with_hash + self.without_hash,
            self.distinct_hashes
        );
        for s in &self.stages {
            let _ = writeln!(
                out,
                "{:<14} {:>6} programs in {:>5} hashes; one header user SGPR count per hash in {}",
                s.stage, s.programs, s.hashes, s.single_header
            );
        }
        for p in &self.pairs {
            let _ = writeln!(
                out,
                "{} -> {}: {} hashes, equal header count in {}, unequal in {}",
                p.from, p.to, p.hashes, p.equal_header, p.unequal_header
            );
            let show = |c: &ShiftCount| {
                format!(
                    "shift {:+}: {} agree, {} differ",
                    c.shift, c.agree, c.differ
                )
            };
            if let Some(best) = p.best() {
                let mut line = format!("    best {}", show(best));
                for s in [0, 8] {
                    if s != best.shift
                        && let Some(c) = p.at(s)
                    {
                        let _ = write!(line, "; {}", show(c));
                    }
                }
                let _ = writeln!(out, "{line}");
            }
        }
        for (stage, agree, differ) in &self.control {
            let _ = writeln!(
                out,
                "control, {stage} programs picked without regard to hash, shift 0: {agree} agree, {differ} differ"
            );
        }
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_the_last_eight_hex_digits_before_the_suffix() {
        assert_eq!(
            technique("PS_Fullscreen_930058ac_Wrapped"),
            Some("930058ac")
        );
        assert_eq!(technique("VS_a_b_c_0123ABCD"), Some("0123ABCD"));
        assert_eq!(technique("PS_Fullscreen_Wrapped"), None);
        assert_eq!(technique("PS_X_930058a_Wrapped"), None);
        assert_eq!(technique("noscore"), None);
    }
}
