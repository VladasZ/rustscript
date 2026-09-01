//! The regex method cores.

use anyhow::{Result, bail};

use super::{Args, usize_arg, usize_i64};
use crate::interpreter::bytecode::BuiltinId;

/// Spans index into the source the caller holds.
pub(crate) enum RegexOut {
    Bool(bool),
    Text(String),
    Pattern,
    OptSpan(Option<(usize, usize)>),
    OptGroups(Option<Vec<Option<(usize, usize)>>>),
    Pieces(Vec<String>),
}

/// The eager methods. `find_iter` and `captures_iter` are lazy and live elsewhere.
pub(crate) fn regex_core(
    re: &regex::Regex,
    name: BuiltinId,
    source: &str,
    args: &impl Args,
) -> Result<Option<RegexOut>> {
    Ok(Some(match name {
        BuiltinId::IsMatch => RegexOut::Bool(re.is_match(source)),
        BuiltinId::Find => RegexOut::OptSpan(re.find(source).map(|m| (m.start(), m.end()))),
        BuiltinId::Captures => RegexOut::OptGroups(re.captures(source).map(|c| {
            (0..c.len())
                .map(|i| c.get(i).map(|g| (g.start(), g.end())))
                .collect()
        })),
        BuiltinId::Replace => {
            RegexOut::Text(re.replacen(source, 1, args.text(1).as_str()).into_owned())
        }
        BuiltinId::ReplaceAll => {
            RegexOut::Text(re.replace_all(source, args.text(1).as_str()).into_owned())
        }
        // the limit comes before the replacement, unlike `str::replacen`
        BuiltinId::Replacen => RegexOut::Text(
            re.replacen(source, usize_arg(args, 1)?, args.text(2).as_str())
                .into_owned(),
        ),
        BuiltinId::Split => RegexOut::Pieces(re.split(source).map(str::to_string).collect()),
        BuiltinId::AsStr => RegexOut::Pattern,
        _ => return Ok(None),
    }))
}

pub(crate) enum MatchOut {
    Text(String),
    Int(i64),
}

pub(crate) fn match_core(
    name: BuiltinId,
    source: &str,
    start: usize,
    end: usize,
) -> Option<MatchOut> {
    Some(match name {
        BuiltinId::AsStr => MatchOut::Text(source[start..end].to_string()),
        BuiltinId::Start => MatchOut::Int(usize_i64(start)),
        BuiltinId::End => MatchOut::Int(usize_i64(end)),
        _ => return None,
    })
}

pub(crate) enum CapturesOut {
    Int(i64),
    OptSpan(Option<(usize, usize)>),
}

pub(crate) fn captures_core<'n>(
    name: BuiltinId,
    groups: &[Option<(usize, usize)>],
    mut names: impl Iterator<Item = (&'n str, usize)>,
    args: &impl Args,
) -> Result<Option<CapturesOut>> {
    Ok(Some(match name {
        BuiltinId::Get => {
            let Some(index) = args.int(0).and_then(|i| usize::try_from(i).ok()) else {
                bail!("captures get needs a non-negative index");
            };
            CapturesOut::OptSpan(groups.get(index).copied().flatten())
        }
        BuiltinId::Name => {
            let wanted = args.text(0);
            let index = names.find_map(|(n, i)| (n == wanted).then_some(i));
            CapturesOut::OptSpan(index.and_then(|i| groups.get(i).copied().flatten()))
        }
        BuiltinId::Len => CapturesOut::Int(usize_i64(groups.len())),
        _ => return Ok(None),
    }))
}
