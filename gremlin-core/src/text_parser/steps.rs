//! `parse_step_token` — the giant per-step dispatcher — plus `parse_has_step` / `parse_by_step`.

use super::*;
use super::{literal::*, predicate::*, tokenizer::*, traversal::*};

pub(crate) fn parse_step_token(
    token: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<(), TextParseError> {
    fn attach_property_meta(base: GremlinValue, meta_pairs: &[GremlinValue]) -> GremlinValue {
        let mut map = match base {
            GremlinValue::Map(map) if map.contains_key("value") => map,
            other => {
                let mut map = IndexMap::new();
                map.insert("value".to_string(), other);
                map
            }
        };
        let mut idx = 0;
        while idx + 1 < meta_pairs.len() {
            if let GremlinValue::String(key) = &meta_pairs[idx] {
                map.insert(key.clone(), meta_pairs[idx + 1].clone());
            }
            idx += 2;
        }
        GremlinValue::Map(map)
    }

    let (name, args_str) = split_step_name_args(token);

    match name {
        "V" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.is_empty() {
                out.push(GremlinStep::V(None));
            } else {
                out.push(GremlinStep::V(Some(args)));
            }
        }
        "E" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.is_empty() {
                out.push(GremlinStep::E(None));
            } else {
                out.push(GremlinStep::E(Some(args)));
            }
        }
        "has" => parse_has_step(args_str, resolve_param, out)?,
        "hasLabel" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::HasLabel(args));
        }
        "hasId" => {
            // Try predicate form first: hasId(P.neq(xx1)), hasId(P.without([]))
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::HasIdPredicate(pred));
            } else {
                let args = parse_arg_list(args_str, resolve_param)?;
                out.push(GremlinStep::HasId(args));
            }
        }
        "hasNot" => {
            // hasNot("key") — filter elements where property does NOT exist
            let args = parse_string_arg_list(args_str)?;
            if let Some(key) = args.into_iter().next() {
                // Wrap in Not(Has(key, None)) to negate the existence check
                out.push(GremlinStep::Not(vec![GremlinStep::Has(key, None)]));
            }
        }
        "out" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::Out(args));
        }
        "in" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::In(args));
        }
        "both" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::Both(args));
        }
        "outE" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::OutE(args));
        }
        "inE" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::InE(args));
        }
        "bothE" => {
            let args = parse_string_arg_list_with_params(args_str, resolve_param)?;
            out.push(GremlinStep::BothE(args));
        }
        "inV" => out.push(GremlinStep::InV),
        "outV" => out.push(GremlinStep::OutV),
        "bothV" => out.push(GremlinStep::BothV),
        "otherV" => out.push(GremlinStep::OtherV),
        "values" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Values(args));
        }
        "valueMap" => {
            let args = parse_string_arg_list(args_str)?;
            // valueMap(true, ...) or valueMap(true) includes tokens (id/label)
            let (token_mode, keys) = if args.first().map(|s| s.as_str()) == Some("true") {
                (1u8, args[1..].to_vec())
            } else {
                (0u8, args)
            };
            out.push(GremlinStep::ValueMap(keys, token_mode));
        }
        "properties" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Properties(args));
        }
        "id" => out.push(GremlinStep::Id),
        "label" => out.push(GremlinStep::Label),
        "select" => {
            let args = parse_string_arg_list(args_str)?;
            // Extract Pop modifier if present. TinkerPop text uses both
            // Pop.first and bare first/last/all/mixed forms.
            let mut pop = None;
            let filtered: Vec<String> = args
                .into_iter()
                .filter(|a| match a.as_str() {
                    "Pop.first" | "first" => {
                        pop = Some(crate::bytecode::Pop::First);
                        false
                    }
                    "Pop.last" | "last" => {
                        pop = Some(crate::bytecode::Pop::Last);
                        false
                    }
                    "Pop.all" | "all" => {
                        pop = Some(crate::bytecode::Pop::All);
                        false
                    }
                    "Pop.mixed" | "mixed" => {
                        pop = Some(crate::bytecode::Pop::Mixed);
                        false
                    }
                    _ => true,
                })
                .map(|a| resolve_constant_traversal(&a))
                .collect();
            out.push(GremlinStep::Select(pop, filtered));
        }
        "as" => {
            let args = parse_string_arg_list(args_str)?;
            for alias in args {
                out.push(GremlinStep::As(alias));
            }
        }
        "count" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                // count(local) — count elements in the list/map value
                out.push(GremlinStep::StringOpLocal("gremlin_count_local".into()));
            } else {
                out.push(GremlinStep::Count);
            }
        }
        "sum" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::SumLocal);
            } else {
                out.push(GremlinStep::Sum);
            }
        }
        "min" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::MinLocal);
            } else {
                out.push(GremlinStep::Min);
            }
        }
        "max" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::MaxLocal);
            } else {
                out.push(GremlinStep::Max);
            }
        }
        "mean" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::MeanLocal);
            } else {
                out.push(GremlinStep::Mean);
            }
        }
        "group" => {
            let args = parse_string_arg_list(args_str)?;
            let key = args.into_iter().next();
            out.push(GremlinStep::Group(key));
        }
        "groupCount" => {
            let args = parse_string_arg_list(args_str)?;
            let key = args.into_iter().next();
            out.push(GremlinStep::GroupCount(key));
        }
        "fold" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.len() >= 2 {
                // fold(seed, Operator.op) — reduce with binary operator
                let seed = args[0].clone();
                let op_str = match &args[1] {
                    GremlinValue::String(s) => s.clone(),
                    _ => "sum".to_string(),
                };
                out.push(GremlinStep::FoldSeed(seed, op_str));
            } else {
                out.push(GremlinStep::Fold);
            }
        }
        "unfold" => out.push(GremlinStep::Unfold),
        "order" => {
            // Detect Scope.local
            if args_str.contains("Scope.local") || args_str.contains("local") {
                out.push(GremlinStep::SortLocal);
            } else {
                out.push(GremlinStep::Order);
            }
        }
        "by" => parse_by_step(args_str, out, resolve_param)?,
        "limit" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            // Detect Scope.local
            let is_local = args.iter().any(
                |a| matches!(a, GremlinValue::String(s) if s == "Scope.local" || s == "local"),
            );
            // limit(n) or limit(Scope.global, n) — skip Scope arg
            let n = args.iter().find_map(|a| {
                if let GremlinValue::Integer(v) = a {
                    Some(*v)
                } else {
                    None
                }
            });
            if let Some(n) = n {
                if is_local {
                    out.push(GremlinStep::LimitLocal(n));
                } else {
                    out.push(GremlinStep::Limit(n));
                }
            }
        }
        "range" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            // Detect Scope.local
            let is_local = args.iter().any(
                |a| matches!(a, GremlinValue::String(s) if s == "Scope.local" || s == "local"),
            );
            // Collect integer args
            let ints: Vec<i64> = args
                .iter()
                .filter_map(|a| {
                    if let GremlinValue::Integer(v) = a {
                        Some(*v)
                    } else {
                        None
                    }
                })
                .collect();
            if ints.len() >= 2 {
                if is_local {
                    out.push(GremlinStep::RangeLocal(ints[0], ints[1]));
                } else {
                    out.push(GremlinStep::Range(ints[0], ints[1]));
                }
            } else if ints.len() == 1 {
                // Single int with scope — not meaningful, ignore
            }
        }
        "dedup" => {
            // Detect Scope.local
            if args_str.contains("Scope.local") || args_str.contains("local") {
                // dedup(Scope.local) or dedup(Scope.local, "x", "y")
                let args: Vec<String> = parse_string_arg_list(args_str)?
                    .into_iter()
                    .filter(|a| a != "Scope.local" && a != "local")
                    .collect();
                out.push(GremlinStep::DedupLocal(args));
            } else {
                let args = parse_string_arg_list(args_str)?;
                out.push(GremlinStep::Dedup(args));
            }
        }
        "is" => {
            // Handle chained predicates: P.eq(1).and(P.gt(0)) or P.eq(1).or(P.lt(0))
            let s = args_str.trim();
            let chain_parts = split_predicate_chain(s);
            if chain_parts.len() > 1 {
                // Parse each part individually
                let mut parsed: Vec<(String, GremlinStep)> = Vec::new();
                for (op, part) in &chain_parts {
                    if let Some(pred) = try_parse_predicate_call(part, resolve_param) {
                        parsed.push((op.to_string(), GremlinStep::Is(pred)));
                    }
                }
                // Group into AND-chains separated by OR operators.
                // P.gt(18).and(P.lt(30)).or(P.gt(35)) → (gt(18) AND lt(30)) OR gt(35)
                let mut or_groups: Vec<Vec<GremlinStep>> = vec![vec![]];
                for (op, step) in parsed {
                    if op == "or" {
                        or_groups.push(vec![step]);
                    } else {
                        or_groups.last_mut().unwrap().push(step);
                    }
                }
                if or_groups.len() == 1 {
                    // All AND — emit as sequential Is steps
                    for step in or_groups.into_iter().flatten() {
                        out.push(step);
                    }
                } else {
                    // Build Or of And groups
                    let mut or_branches: Vec<Vec<GremlinStep>> = Vec::new();
                    for group in or_groups {
                        if group.len() == 1 {
                            or_branches.push(group);
                        } else {
                            // Multiple AND predicates become a single branch
                            or_branches.push(group);
                        }
                    }
                    out.push(GremlinStep::Or(or_branches));
                }
            } else {
                let pred = parse_predicate(args_str, resolve_param)?;
                out.push(GremlinStep::Is(pred));
            }
        }
        "where" => {
            // where(P.neq("a")) or where("a", P.eq("b")) or where(__.out("knows"))
            let raw_args = split_top_level_args(args_str);
            if raw_args.len() == 2 {
                // where("a", P.eq("b")) — label comparison
                let first = raw_args[0].trim();
                let second = raw_args[1].trim();
                let label = unquote(first);
                if let Some(pred) = try_parse_predicate_call(second, resolve_param) {
                    // Convert to: select("a").is(pred) effectively
                    // Emit as Where with a select + is pattern
                    out.push(GremlinStep::Where(vec![
                        GremlinStep::Select(None, vec![label]),
                        GremlinStep::Is(pred),
                    ]));
                } else {
                    let inner_steps = parse_inner_traversal(args_str, resolve_param)?;
                    out.push(GremlinStep::Where(inner_steps));
                }
            } else {
                let inner_steps = parse_inner_traversal(args_str, resolve_param)?;
                out.push(GremlinStep::Where(inner_steps));
            }
        }
        "not" => {
            let inner_steps = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Not(inner_steps));
        }
        "and" => {
            let inner_traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            out.push(GremlinStep::And(inner_traversals));
        }
        "or" => {
            let inner_traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            out.push(GremlinStep::Or(inner_traversals));
        }
        "addV" => {
            if args_str.contains('(') {
                let inner = parse_inner_traversal(args_str, resolve_param)?;
                if !inner.is_empty() {
                    out.push(GremlinStep::AddVTraversal(inner));
                } else {
                    let args = parse_string_arg_list(args_str)?;
                    let label = args.into_iter().next().unwrap_or_default();
                    out.push(GremlinStep::AddV(label));
                }
            } else {
                let args = parse_string_arg_list(args_str)?;
                let label = args.into_iter().next().unwrap_or_default();
                out.push(GremlinStep::AddV(label));
            }
        }
        "addE" => {
            let args = parse_string_arg_list(args_str)?;
            let label = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::AddE(label));
        }
        "property" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.len() >= 2 {
                // Check for property(Cardinality.xxx, ...) pattern
                let is_cardinality = matches!(&args[0],
                    GremlinValue::String(s) if s.starts_with("Cardinality.") || s == "single" || s == "list" || s == "set"
                );
                if is_cardinality && matches!(&args[1], GremlinValue::Map(_)) {
                    // property(Cardinality.xxx, Map) — expand Map entries
                    if let GremlinValue::Map(map) = &args[1] {
                        for (k, v) in map {
                            out.push(GremlinStep::Property(k.clone(), v.clone()));
                        }
                    }
                } else if is_cardinality && args.len() >= 3 {
                    // property(Cardinality.xxx, key, value) — skip cardinality
                    if let GremlinValue::String(key) = &args[1] {
                        let value = if args.len() > 3 {
                            attach_property_meta(args[2].clone(), &args[3..])
                        } else {
                            args[2].clone()
                        };
                        out.push(GremlinStep::Property(key.clone(), value));
                    }
                } else if let GremlinValue::String(key) = &args[0] {
                    let value = if args.len() > 2 {
                        attach_property_meta(args[1].clone(), &args[2..])
                    } else {
                        args[1].clone()
                    };
                    out.push(GremlinStep::Property(key.clone(), value));
                }
            } else if args.len() == 1 {
                // property(Map) — expand Map entries into individual property steps
                if let GremlinValue::Map(map) = &args[0] {
                    for (k, v) in map {
                        out.push(GremlinStep::Property(k.clone(), v.clone()));
                    }
                }
            }
        }
        "from" => {
            let inner = parse_from_to_traversal(args_str, resolve_param)?;
            // Direction.OUT/IN/BOTH produces direct traversal steps —
            // emit them without the From() wrapper.
            if inner.len() == 1
                && matches!(
                    &inner[0],
                    GremlinStep::Out(_) | GremlinStep::In(_) | GremlinStep::Both(_)
                )
            {
                out.extend(inner);
            } else {
                out.push(GremlinStep::From(inner));
            }
        }
        "to" => {
            let inner = parse_from_to_traversal(args_str, resolve_param)?;
            // Direction.OUT/IN/BOTH produces direct traversal steps —
            // emit them without the To() wrapper.
            if inner.len() == 1
                && matches!(
                    &inner[0],
                    GremlinStep::Out(_) | GremlinStep::In(_) | GremlinStep::Both(_)
                )
            {
                out.extend(inner);
            } else {
                out.push(GremlinStep::To(inner));
            }
        }
        "drop" => out.push(GremlinStep::Drop),
        "path" => out.push(GremlinStep::Path),
        "repeat" => {
            // repeat(body) or repeat("label", body)
            let trimmed = args_str.trim();
            if trimmed.starts_with('"') || trimmed.starts_with('\'') {
                // repeat("label", body) — extract the label and body
                let quote = trimmed.as_bytes()[0] as char;
                if let Some(end_quote) = trimmed[1..].find(quote) {
                    let label = &trimmed[1..1 + end_quote];
                    let rest = trimmed[1 + end_quote + 1..].trim();
                    let rest = rest.strip_prefix(',').unwrap_or(rest).trim();
                    let inner = parse_inner_traversal(rest, resolve_param)?;
                    out.push(GremlinStep::Repeat(Some(label.into()), inner));
                } else {
                    let inner = parse_inner_traversal(args_str, resolve_param)?;
                    out.push(GremlinStep::Repeat(None, inner));
                }
            } else {
                let inner = parse_inner_traversal(args_str, resolve_param)?;
                out.push(GremlinStep::Repeat(None, inner));
            }
        }
        "times" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if let Some(GremlinValue::Integer(n)) = args.first() {
                out.push(GremlinStep::Times(*n));
            }
        }
        "until" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Until(inner));
        }
        "emit" => {
            let trimmed = args_str.trim();
            if trimmed.is_empty() {
                out.push(GremlinStep::Emit);
            } else {
                let inner = parse_inner_traversal(trimmed, resolve_param)?;
                if inner.is_empty() {
                    out.push(GremlinStep::Emit);
                } else {
                    out.push(GremlinStep::EmitPred(inner));
                }
            }
        }
        "aggregate" => {
            let args = parse_string_arg_list(args_str)?;
            // aggregate("x") or aggregate(local, "x") / aggregate(Scope.local, "x")
            let mut iter = args.into_iter();
            if let Some(first) = iter.next() {
                if first == "local"
                    || first == "Scope.local"
                    || first == "global"
                    || first == "Scope.global"
                {
                    // Scope argument — use the next arg as key
                    if let Some(key) = iter.next() {
                        out.push(GremlinStep::Aggregate(key));
                    }
                } else {
                    out.push(GremlinStep::Aggregate(first));
                }
            }
        }
        "cap" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Cap(args));
        }
        "constant" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let val = args.into_iter().next().unwrap_or(GremlinValue::Null);
            out.push(GremlinStep::Constant(val));
        }
        "inject" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            out.push(GremlinStep::Inject(args));
        }
        "project" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Project(args));
        }
        "match" => {
            let traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            out.push(GremlinStep::Match(traversals));
        }
        "union" => {
            let traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            out.push(GremlinStep::Union(traversals));
        }
        "coalesce" => {
            let traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            out.push(GremlinStep::Coalesce(traversals));
        }
        "choose" => {
            let traversals = parse_multi_traversal_args(args_str, resolve_param)?;
            match traversals.len() {
                3 => out.push(GremlinStep::Choose(
                    traversals[0].clone(),
                    traversals[1].clone(),
                    Some(traversals[2].clone()),
                )),
                2 => out.push(GremlinStep::Choose(
                    traversals[0].clone(),
                    traversals[1].clone(),
                    None,
                )),
                1 => {
                    // 1-arg choose: value-based dispatch with option() modifiers
                    out.push(GremlinStep::Choose(traversals[0].clone(), Vec::new(), None));
                }
                _ => {
                    out.push(GremlinStep::Choose(Vec::new(), Vec::new(), None));
                }
            }
        }
        "sideEffect" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::SideEffect(inner));
        }
        "identity" => out.push(GremlinStep::Identity),
        "tail" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            // Detect Scope.local
            let has_local_scope = args_str.contains("local") || args_str.contains("Scope.local");
            let n = args
                .iter()
                .find_map(|a| {
                    if let GremlinValue::Integer(v) = a {
                        Some(*v)
                    } else {
                        None
                    }
                })
                .unwrap_or(1);
            if has_local_scope {
                out.push(GremlinStep::TailLocal(n));
            } else {
                out.push(GremlinStep::Tail(n));
            }
        }
        "skip" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let is_local = args.iter().any(
                |a| matches!(a, GremlinValue::String(s) if s == "Scope.local" || s == "local"),
            );
            let n = args.iter().find_map(|a| {
                if let GremlinValue::Integer(v) = a {
                    Some(*v)
                } else {
                    None
                }
            });
            if let Some(n) = n {
                if is_local {
                    out.push(GremlinStep::SkipLocal(n));
                } else {
                    out.push(GremlinStep::Skip(n));
                }
            }
        }
        "elementMap" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::ElementMap(args));
        }
        "hasKey" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::HasKey(args));
        }
        "hasValue" => {
            // Try predicate first: hasValue(P.gt(30))
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::HasValuePredicate(pred));
            } else {
                let args = parse_arg_list(args_str, resolve_param)?;
                out.push(GremlinStep::HasValue(args));
            }
        }
        "optional" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Optional(inner));
        }
        "simplePath" => out.push(GremlinStep::SimplePath),
        "cyclicPath" => out.push(GremlinStep::CyclicPath),
        "coin" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let p = match args.first() {
                Some(GremlinValue::Float(f)) => *f,
                Some(GremlinValue::Integer(i)) => *i as f64,
                _ => 0.5,
            };
            out.push(GremlinStep::Coin(p));
        }
        "map" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Map(inner));
        }
        "flatMap" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::FlatMap(inner));
        }
        "filter" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::FilterTraversal(inner));
        }
        "none" => {
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::NonePred(pred));
            } else {
                out.push(GremlinStep::None);
            }
        }
        "barrier" => {
            if args_str.contains("normSack") || args_str.contains("Barrier.normSack") {
                out.push(GremlinStep::BarrierNormSack);
            } else {
                out.push(GremlinStep::Barrier);
            }
        }
        "sample" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let is_local = args.iter().any(
                |a| matches!(a, GremlinValue::String(s) if s == "Scope.local" || s == "local"),
            );
            let n = args
                .iter()
                .find_map(|a| {
                    if let GremlinValue::Integer(v) = a {
                        Some(*v)
                    } else {
                        None
                    }
                })
                .unwrap_or(1);
            if is_local {
                out.push(GremlinStep::SampleLocal(n));
            } else {
                out.push(GremlinStep::Sample(n));
            }
        }
        "discard" => out.push(GremlinStep::Discard),
        "loops" => {
            // loops() or loops("label")
            let trimmed = args_str.trim();
            if trimmed.starts_with('"') || trimmed.starts_with('\'') {
                let quote = trimmed.as_bytes()[0] as char;
                if let Some(end_quote) = trimmed[1..].find(quote) {
                    let label = &trimmed[1..1 + end_quote];
                    out.push(GremlinStep::Loops(Some(label.into())));
                } else {
                    out.push(GremlinStep::Loops(None));
                }
            } else {
                out.push(GremlinStep::Loops(None));
            }
        }
        "local" => {
            // local(traversal) — apply traversal to each element locally
            // Semantically similar to map(traversal)
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Map(inner));
        }
        "option" => {
            // option(key, traversal) — modifier for choose/branch
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.len() >= 2 {
                // First arg: the option key (Pick.none maps to "none" string)
                let token = args[0].clone();
                // Reject bare traversal steps as Pick tokens (e.g., option(is("marko"), ...)).
                // Only predicates (P.eq, P.neq, etc.), values, and __.traversal() are allowed.
                // Note: __.prefixed traversals (like __.hasLabel("software")) are valid for branch().
                if let GremlinValue::String(ref s) = token {
                    let lower = s.to_lowercase();
                    if !s.starts_with("__")
                        && (lower.starts_with("is(")
                            || lower.starts_with("haslabel(")
                            || lower.starts_with("has(")
                            || lower.starts_with("out(")
                            || lower.starts_with("in(")
                            || lower.starts_with("both(")
                            || lower.starts_with("values(")
                            || lower.starts_with("identity("))
                    {
                        return Err(TextParseError::Expected {
                            expected: "Traversal is not allowed as a Pick token".into(),
                            found: s.clone(),
                        });
                    }
                }
                // Remaining args contain the traversal steps.
                // Parse the original args_str to extract the traversal part.
                let inner_traversal = parse_option_traversal(args_str, resolve_param)?;
                out.push(GremlinStep::Option(token, inner_traversal));
            } else if args.len() == 1 {
                // option(Pick.none, discard) might parse as single arg
                let token = args[0].clone();
                out.push(GremlinStep::Option(token, vec![GremlinStep::Identity]));
            }
        }
        "value" => {
            // value() — extract the value from a property
            // Equivalent to values() with no keys on a property element
            out.push(GremlinStep::Values(vec![]));
        }
        "key" => {
            // key() — extract the key from a property
            out.push(GremlinStep::Label);
        }
        "store" => {
            // store("key") — like aggregate but uses lazy semantics
            let args = parse_string_arg_list(args_str)?;
            if let Some(key) = args.into_iter().next() {
                out.push(GremlinStep::Aggregate(key));
            }
        }
        "branch" => {
            // branch(traversal) — general branch step; choose is the more common form
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::Choose(inner, Vec::new(), None));
        }
        "sack" => {
            let args_trimmed = args_str.trim();
            if args_trimmed.is_empty() {
                // sack() — returns the sack value
                out.push(GremlinStep::Sack);
            } else {
                // sack(operator) — apply binary op
                let op_name = args_trimmed.replace("Operator.", "").trim().to_lowercase();
                out.push(GremlinStep::SackOp(op_name));
            }
        }
        "with" | "withPath" | "withBulk" => {
            // with(WithOptions.tokens[, WithOptions.labels|ids]) — set token mode
            let args = parse_string_arg_list(args_str)?;
            if args.iter().any(|a| a == "WithOptions.tokens") {
                let mode = if args.iter().any(|a| a == "WithOptions.labels") {
                    2u8 // labels only
                } else if args.iter().any(|a| a == "WithOptions.ids") {
                    3u8 // ids only
                } else {
                    1u8 // all tokens
                };
                for step in out.iter_mut().rev() {
                    if let GremlinStep::ValueMap(_, ref mut token_mode) = step {
                        *token_mode = mode;
                        break;
                    }
                }
            }
            // with(WithOptions.indexer, WithOptions.map) — convert index to map mode
            if args.iter().any(|a| a == "WithOptions.indexer")
                && args.iter().any(|a| a == "WithOptions.map")
            {
                for step in out.iter_mut().rev() {
                    if matches!(step, GremlinStep::Index) {
                        *step = GremlinStep::IndexWithMap;
                        break;
                    }
                }
            }
            // withPath — enable path tracking
            // withBulk — enable bulk optimization
        }
        "element" => {
            // element() — return the owning element from a property
            out.push(GremlinStep::Element);
        }
        "math" => {
            // math("expression") — mathematical expression on traversal values
            let args = parse_string_arg_list(args_str)?;
            let expr_str = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Math(expr_str));
        }
        "index" => {
            // index() — adds index to each element
            out.push(GremlinStep::Index);
        }
        // ── String steps ────────────────────────────────────────────────────
        "toLower" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_tolower_local".into()));
            } else {
                out.push(GremlinStep::ToLower);
            }
        }
        "toUpper" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_toupper_local".into()));
            } else {
                out.push(GremlinStep::ToUpper);
            }
        }
        "trim" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_trim_local".into()));
            } else {
                out.push(GremlinStep::Trim);
            }
        }
        "lTrim" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_ltrim_local".into()));
            } else {
                out.push(GremlinStep::LTrim);
            }
        }
        "rTrim" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_rtrim_local".into()));
            } else {
                out.push(GremlinStep::RTrim);
            }
        }
        "length" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_length_local".into()));
            } else {
                out.push(GremlinStep::Length);
            }
        }
        "reverse" => {
            out.push(GremlinStep::Reverse);
        }
        "replace" => {
            let args = parse_string_arg_list(args_str)?;
            let is_local = args
                .first()
                .map(|a| a.contains("local") || a == "Scope.local")
                .unwrap_or(false);
            if is_local {
                // replace(local, old, new)
                if args.len() >= 3 {
                    out.push(GremlinStep::StringOpLocal(format!(
                        "gremlin_replace_local\x00{}\x00{}",
                        args[1], args[2]
                    )));
                }
            } else if args.len() >= 2 {
                out.push(GremlinStep::Replace(args[0].clone(), args[1].clone()));
            } else {
                return Err(TextParseError::InvalidArgument(
                    "replace requires 2 arguments".into(),
                ));
            }
        }
        "split" => {
            let args = parse_string_arg_list(args_str)?;
            let is_local = args
                .first()
                .map(|a| a.contains("local") || a == "Scope.local")
                .unwrap_or(false);
            if is_local {
                // split(local, sep)
                if args.len() >= 2 {
                    out.push(GremlinStep::StringOpLocal(format!(
                        "gremlin_split_local\x00{}",
                        args[1]
                    )));
                }
            } else if !args.is_empty() {
                out.push(GremlinStep::Split(args[0].clone()));
            } else {
                // split("") — split each character
                out.push(GremlinStep::Split(String::new()));
            }
        }
        "substring" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            // Check if first arg is Scope.local
            let (offset, is_local) = if matches!(args.first(), Some(GremlinValue::String(s)) if s.contains("local"))
            {
                (1, true)
            } else {
                (0, false)
            };
            let start = match args.get(offset) {
                Some(GremlinValue::Integer(n)) => *n,
                _ => 0,
            };
            let len = match args.get(offset + 1) {
                Some(GremlinValue::Integer(n)) => Some(*n),
                _ => None,
            };
            if is_local {
                out.push(GremlinStep::SubstringLocal(start, len));
            } else {
                out.push(GremlinStep::Substring(start, len));
            }
        }
        "concat" => {
            // concat() args can be string literals, null, or traversals
            let raw_args = split_top_level_args(args_str);
            let mut traversals = Vec::new();
            for arg in &raw_args {
                let s = arg.trim();
                if s.is_empty() {
                    continue;
                }
                // String literal: "c"
                if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                    let inner = &s[1..s.len() - 1];
                    let unescaped = inner.replace("\\\"", "\"").replace("\\\\", "\\");
                    traversals.push(vec![GremlinStep::Constant(GremlinValue::String(unescaped))]);
                } else if s == "null" {
                    traversals.push(vec![GremlinStep::Constant(GremlinValue::Null)]);
                } else {
                    // Sub-traversal
                    let inner = parse_inner_traversal(s, resolve_param)?;
                    traversals.push(inner);
                }
            }
            out.push(GremlinStep::Concat(traversals));
        }
        "conjoin" => {
            let args = parse_string_arg_list(args_str)?;
            let sep = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Conjoin(sep));
        }
        "format" => {
            let args = parse_string_arg_list(args_str)?;
            let pattern = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Format(pattern));
        }
        "asString" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::AsStringLocal);
            } else {
                out.push(GremlinStep::AsString);
            }
        }
        "asNumber" => {
            let trimmed = args_str.trim();
            if trimmed.is_empty() {
                out.push(GremlinStep::AsNumber);
            } else if trimmed.contains("INT") || trimmed.contains("LONG") {
                out.push(GremlinStep::AsNumberTyped("int".into()));
            } else if trimmed.contains("FLOAT")
                || trimmed.contains("DOUBLE")
                || trimmed.contains("BIG_DECIMAL")
            {
                out.push(GremlinStep::AsNumberTyped("float".into()));
            } else if trimmed.contains("BYTE") {
                out.push(GremlinStep::AsNumberTyped("byte".into()));
            } else if trimmed.contains("SHORT") {
                out.push(GremlinStep::AsNumberTyped("short".into()));
            } else if trimmed.contains("VERTEX")
                || trimmed.contains("EDGE")
                || trimmed.contains("PROPERTY")
                || trimmed.contains("PATH")
            {
                out.push(GremlinStep::AsNumberTyped("__invalid__".into()));
            } else {
                out.push(GremlinStep::AsNumber);
            }
        }
        "asBool" => {
            out.push(GremlinStep::AsBool);
        }
        "typeOf" => {
            out.push(GremlinStep::TypeOf);
        }
        // ── Collection steps ────────────────────────────────────────────────
        "combine" => {
            let inner = parse_collection_step_arg(args_str, resolve_param)?;
            out.push(GremlinStep::Combine(inner));
        }
        "intersect" => {
            let inner = parse_collection_step_arg(args_str, resolve_param)?;
            out.push(GremlinStep::Intersect(inner));
        }
        "difference" => {
            let inner = parse_collection_step_arg(args_str, resolve_param)?;
            out.push(GremlinStep::Difference(inner));
        }
        "disjunct" => {
            let inner = parse_collection_step_arg(args_str, resolve_param)?;
            out.push(GremlinStep::Disjunct(inner));
        }
        "merge" => {
            let inner = parse_collection_step_arg(args_str, resolve_param)?;
            out.push(GremlinStep::MergeList(inner));
        }
        "product" => {
            let inner = parse_collection_step_arg(args_str, resolve_param)?;
            out.push(GremlinStep::Product(inner));
        }
        "all" => {
            // all(P.gt(0)) — all elements match predicate
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::All(pred));
            } else {
                out.push(GremlinStep::All(Predicate::Neq(GremlinValue::Null)));
            }
        }
        "any" => {
            // any(P.gt(0)) — any element matches predicate
            if let Some(pred) = try_parse_predicate_call(args_str.trim(), resolve_param) {
                out.push(GremlinStep::Any(pred));
            } else {
                out.push(GremlinStep::Any(Predicate::Neq(GremlinValue::Null)));
            }
        }
        // Steps still unsupported (provider-specific or not applicable)
        "profile" | "iterate" | "explain" | "fail" => {
            return Err(TextParseError::UnknownStep(name.to_string()));
        }
        // Unsupported but parseable — compile to CompileError::UnsupportedStep
        "read" | "write" => {
            out.push(GremlinStep::Io(String::new()));
        }
        "tree" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Tree(args.into_iter().next()));
        }
        "subgraph" => {
            let args = parse_string_arg_list(args_str)?;
            let key = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Subgraph(key));
        }
        "call" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            out.push(GremlinStep::Call(args));
        }
        "shortestPath" => {
            out.push(GremlinStep::ShortestPath);
        }
        "pageRank" => {
            out.push(GremlinStep::PageRank);
        }
        "connectedComponent" => {
            out.push(GremlinStep::ConnectedComponent);
        }
        "peerPressure" => {
            out.push(GremlinStep::PeerPressure);
        }
        "io" => {
            let args = parse_string_arg_list(args_str)?;
            let file = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Io(file));
        }
        "asDate" => {
            out.push(GremlinStep::AsDate);
        }
        "dateAdd" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let unit = match args.first() {
                Some(GremlinValue::String(s)) => s.clone(),
                _ => String::new(),
            };
            let amount = match args.get(1) {
                Some(GremlinValue::Integer(n)) => *n,
                _ => 0,
            };
            out.push(GremlinStep::DateAdd(unit, amount));
        }
        "dateDiff" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::DateDiff(inner));
        }
        "toE" => {
            // toE(Direction.OUT, "label") → outE("label"), etc.
            let raw_args = split_top_level_args(args_str);
            if !raw_args.is_empty() {
                let first = raw_args[0].trim();
                if first.starts_with("Direction.")
                    || first == "OUT"
                    || first == "IN"
                    || first == "BOTH"
                {
                    let labels: Vec<String> =
                        raw_args[1..].iter().map(|a| unquote(a.trim())).collect();
                    // Resolve parameters in labels
                    let labels: Vec<String> = labels
                        .into_iter()
                        .map(|l| {
                            if let Some(GremlinValue::String(s)) = resolve_param(&l) {
                                s
                            } else {
                                l
                            }
                        })
                        .collect();
                    match first {
                        "Direction.OUT" | "OUT" => out.push(GremlinStep::OutE(labels)),
                        "Direction.IN" | "IN" => out.push(GremlinStep::InE(labels)),
                        "Direction.BOTH" | "BOTH" => out.push(GremlinStep::BothE(labels)),
                        _ => out.push(GremlinStep::BothE(labels)),
                    }
                } else {
                    let args = parse_arg_list(args_str, resolve_param)?;
                    out.push(GremlinStep::ToE(args));
                }
            } else {
                let args = parse_arg_list(args_str, resolve_param)?;
                out.push(GremlinStep::ToE(args));
            }
        }
        "mergeV" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let search = args
                .into_iter()
                .next()
                .unwrap_or(GremlinValue::Map(IndexMap::new()));
            out.push(GremlinStep::MergeV(search));
        }
        "mergeE" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let search = args
                .into_iter()
                .next()
                .unwrap_or(GremlinValue::Map(IndexMap::new()));
            out.push(GremlinStep::MergeE(search));
        }
        _ => {
            return Err(TextParseError::UnknownStep(name.to_string()));
        }
    }
    Ok(())
}

/// Split `has("name","marko")` into name="has", args_str=`"name","marko"`.
pub(crate) fn parse_has_step(
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<(), TextParseError> {
    let raw_args = split_top_level_args(args_str);

    match raw_args.len() {
        0 => {
            return Err(TextParseError::InvalidArgument(
                "has() requires arguments".into(),
            ))
        }
        1 => {
            // has("key") — existence check
            let key = parse_single_value(raw_args[0].trim(), resolve_param)?;
            match key {
                GremlinValue::String(k) => {
                    out.push(GremlinStep::Has(k, None));
                }
                GremlinValue::Null => {
                    // has(null) — nothing has a null key, filter all
                    out.push(GremlinStep::Is(Predicate::Eq(GremlinValue::Null)));
                }
                _ => {}
            }
        }
        2 => {
            let key = parse_single_value(raw_args[0].trim(), resolve_param)?;
            let val_str = raw_args[1].trim();

            // has(null, ...) — always false
            if matches!(key, GremlinValue::Null) {
                out.push(GremlinStep::Is(Predicate::Eq(GremlinValue::Null)));
                return Ok(());
            }

            // has(T.label, X) → hasLabel(X)
            if let GremlinValue::String(ref k) = key {
                if k == "~label" {
                    if let Some(pred) = try_parse_predicate_call(val_str, resolve_param) {
                        // has(T.label, P.eq("person")) → hasLabel("person")
                        if let Predicate::Eq(GremlinValue::String(label)) = pred {
                            out.push(GremlinStep::HasLabel(vec![label]));
                        } else {
                            out.push(GremlinStep::HasLabel(vec![]));
                        }
                    } else {
                        let val = parse_single_value(val_str, resolve_param)?;
                        if let GremlinValue::String(label) = val {
                            out.push(GremlinStep::HasLabel(vec![label]));
                        } else if matches!(val, GremlinValue::Null) {
                            // has(T.label, null) — nothing matches null label
                            out.push(GremlinStep::HasLabel(vec![
                                "__null_label_no_match__".to_string()
                            ]));
                        }
                    }
                    return Ok(());
                }
                if k == "~id" {
                    let val = parse_single_value(val_str, resolve_param)?;
                    out.push(GremlinStep::HasId(vec![val]));
                    return Ok(());
                }
            }

            // Check if second arg is a predicate (P.gt, P.within, TextP.*, etc.)
            if let Some(pred) = try_parse_predicate_call(val_str, resolve_param) {
                if let GremlinValue::String(k) = key {
                    out.push(GremlinStep::HasPredicate(k, pred));
                }
            } else {
                let val = parse_single_value(val_str, resolve_param)?;
                if let GremlinValue::String(k) = key {
                    out.push(GremlinStep::Has(k, Some(val)));
                }
            }
        }
        3 => {
            // has(label, key, value) or has(label, key, P.*)
            let label = parse_single_value(raw_args[0].trim(), resolve_param)?;
            let key = parse_single_value(raw_args[1].trim(), resolve_param)?;
            if let (GremlinValue::String(l), GremlinValue::String(k)) = (&label, &key) {
                out.push(GremlinStep::HasLabel(vec![l.clone()]));
                let val_str = raw_args[2].trim();
                if let Some(pred) = try_parse_predicate_call(val_str, resolve_param) {
                    out.push(GremlinStep::HasPredicate(k.clone(), pred));
                } else {
                    let val = parse_single_value(val_str, resolve_param)?;
                    out.push(GremlinStep::Has(k.clone(), Some(val)));
                }
            }
        }
        _ => {
            // Best-effort: treat first as key
            let key = parse_single_value(raw_args[0].trim(), resolve_param)?;
            if let GremlinValue::String(k) = key {
                out.push(GremlinStep::Has(k, None));
            }
        }
    }
    Ok(())
}

/// Parse a `by(...)` modifier.
pub(crate) fn parse_by_step(
    args_str: &str,
    out: &mut Vec<GremlinStep>,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Result<(), TextParseError> {
    let raw_args = split_top_level_args(args_str);

    if raw_args.is_empty() {
        // by() with no args — identity projection
        out.push(GremlinStep::By(String::new(), false));
        return Ok(());
    }

    let first = raw_args[0].trim();

    // Handle single-arg by(Order.desc) or by(Order.asc) — sort current value
    if first == "Order.desc" || first == "desc" {
        out.push(GremlinStep::By(String::new(), true));
        return Ok(());
    }
    if first == "Order.asc" || first == "asc" {
        out.push(GremlinStep::By(String::new(), false));
        return Ok(());
    }
    if first == "Order.shuffle" || first == "shuffle" {
        out.push(GremlinStep::By(String::new(), false));
        return Ok(());
    }

    // Handle T.label, T.id
    if first == "T.label" || first == "t[label]" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~label".to_string(), descending));
        return Ok(());
    }
    if first == "T.id" || first == "t[id]" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~id".to_string(), descending));
        return Ok(());
    }

    // Handle T.key — for ordering by property key name
    if first == "T.key" || first == "t[key]" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~key".to_string(), descending));
        return Ok(());
    }
    // Handle T.value — for ordering by property value
    if first == "T.value" || first == "t[value]" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~value".to_string(), descending));
        return Ok(());
    }

    // Handle Column.keys, Column.values
    if first == "Column.keys" || first == "Column.values" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By(first.to_string(), descending));
        return Ok(());
    }

    // Handle bare token references: by(label) = by(T.label), by(id) = by(T.id)
    if first == "label" || first == "Label" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~label".to_string(), descending));
        return Ok(());
    }
    if first == "id" || first == "Id" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~id".to_string(), descending));
        return Ok(());
    }
    if first == "key" || first == "Key" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~key".to_string(), descending));
        return Ok(());
    }
    if first == "value" || first == "Value" {
        let descending = raw_args.len() >= 2 && {
            let second = raw_args[1].trim();
            second == "Order.desc" || second == "desc"
        };
        out.push(GremlinStep::By("~value".to_string(), descending));
        return Ok(());
    }

    // Check for Order.asc, Order.desc, Order.shuffle as second arg
    let descending = if raw_args.len() >= 2 {
        let second = raw_args[1].trim();
        second == "Order.desc" || second == "desc"
    } else {
        false
    };

    // Detect sub-traversal arguments: if the first arg looks like a traversal
    // (contains function calls like values(...), out(), label(), etc.)
    // parse it as a ByTraversal.
    if looks_like_traversal(first) {
        let inner = parse_inner_traversal(first, resolve_param)?;
        if !inner.is_empty() {
            out.push(GremlinStep::ByTraversal(inner, descending));
            return Ok(());
        }
    }

    let first_unquoted = unquote(first);

    out.push(GremlinStep::By(first_unquoted, descending));
    Ok(())
}
