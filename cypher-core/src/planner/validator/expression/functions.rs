//! Function-name and arity validation.

use crate::ast::Expr;
use crate::error::GqlError;

use crate::planner::validator::plan_error_semantic;

pub(in crate::planner::validator) fn validate_known_function(name: &str) -> Result<(), GqlError> {
    let lower = name.to_ascii_lowercase();
    let known = matches!(
        lower.as_str(),
        "abs"
            | "ceil"
            | "floor"
            | "round"
            | "sqrt"
            | "sign"
            | "tostring"
            | "tointeger"
            | "toint"
            | "tofloat"
            | "toboolean"
            | "size"
            | "length"
            | "toupper"
            | "tolower"
            | "trim"
            | "ltrim"
            | "rtrim"
            | "left"
            | "right"
            | "split"
            | "replace"
            | "substring"
            | "head"
            | "last"
            | "tail"
            | "reverse"
            | "coalesce"
            | "isnull"
            | "range"
            | "type"
            | "keys"
            | "labels"
            | "nodes"
            | "relationships"
            | "id"
            | "properties"
            | "startnode"
            | "endnode"
            | "exists"
            | "rand"
            | "timestamp"
            | "pi"
            | "e"
            | "log"
            | "log10"
            | "exp"
            | "sin"
            | "cos"
            | "tan"
            | "asin"
            | "acos"
            | "atan"
            | "atan2"
            | "degrees"
            | "radians"
            | "haversin"
            | "count"
            | "sum"
            | "avg"
            | "min"
            | "max"
            | "collect"
            | "stdev"
            | "stdevp"
            | "percentilecont"
            | "percentiledisc"
            | "single"
            | "any"
            | "all"
            | "none"
            | "reduce"
            | "extract"
            | "filter"
            | "tobooleanlist"
            | "tointegerlist"
            | "tofloatlist"
            | "tostringlist"
            | "tobooleanornull"
            | "tointegerornull"
            | "tofloatornull"
            | "tostringornull"
            | "char_length"
            | "character_length"
            // Temporal functions
            | "date"
            | "datetime"
            | "localdatetime"
            | "time"
            | "localtime"
            | "duration"
            // Temporal namespaced functions
            | "date.truncate"
            | "datetime.truncate"
            | "localdatetime.truncate"
            | "localtime.truncate"
            | "time.truncate"
            | "date.realtime"
            | "date.statement"
            | "date.transaction"
            | "datetime.realtime"
            | "datetime.statement"
            | "datetime.transaction"
            | "datetime.fromepoch"
            | "datetime.fromepochmillis"
            | "localdatetime.realtime"
            | "localdatetime.statement"
            | "localdatetime.transaction"
            | "localtime.realtime"
            | "localtime.statement"
            | "localtime.transaction"
            | "time.realtime"
            | "time.statement"
            | "time.transaction"
            | "duration.between"
            | "duration.inmonths"
            | "duration.indays"
            | "duration.inseconds"
            // Spatial functions
            | "point"
            | "distance"
    );
    if !known {
        return Err(plan_error_semantic(&format!("unknown function '{name}'")));
    }
    Ok(())
}

pub(in crate::planner::validator) fn validate_function_arity(
    name: &str,
    args: &[Expr],
) -> Result<(), GqlError> {
    let n = args.len();
    let lower_name = name.to_ascii_lowercase();
    let expected = match lower_name.as_str() {
        // Functions that take exactly 0 arguments
        "rand" | "timestamp" | "pi" | "e" => Some((0, 0)),
        // Functions that take exactly 1 argument
        "abs" | "ceil" | "floor" | "round" | "sqrt" | "sign" | "tostring" | "tointeger"
        | "toint" | "tofloat" | "toboolean" | "size" | "length" | "toupper" | "tolower"
        | "trim" | "ltrim" | "rtrim" | "head" | "last" | "tail" | "reverse" | "isnull" | "type"
        | "keys" | "labels" | "nodes" | "relationships" | "id" | "properties" | "startnode"
        | "endnode" | "exists" | "log" | "log10" | "exp" | "sin" | "cos" | "tan" | "asin"
        | "acos" | "atan" | "degrees" | "radians" | "haversin" => Some((1, 1)),
        // Functions that take exactly 2 arguments
        "left" | "right" | "split" | "atan2" => Some((2, 2)),
        // Functions that take exactly 3 arguments
        "replace" => Some((3, 3)),
        // Functions that take 2-3 arguments
        "substring" => Some((2, 3)),
        // Functions that take variable arguments
        "range" => Some((2, 3)),
        "coalesce" => {
            if n == 0 {
                return Err(plan_error_semantic(
                    "coalesce() requires at least 1 argument",
                ));
            }
            None
        }
        // Temporal functions: 0 or 1 argument
        "date" | "datetime" | "localdatetime" | "time" | "localtime" | "duration" => Some((0, 1)),
        // Spatial functions
        "point" => Some((1, 1)),
        "distance" => Some((2, 2)),
        // Aggregation functions: 1 arg (or 0 for count(*))
        "count" => Some((0, 1)),
        "sum" | "avg" | "collect" | "stdev" | "stdevp" | "min" | "max" => Some((1, 1)),
        "percentilecont" | "percentiledisc" => Some((2, 2)),
        _ => None, // Unknown functions are handled elsewhere
    };
    if let Some((min, max)) = expected {
        if n < min || n > max {
            return Err(plan_error_semantic(&format!(
                "function {name}() expects {}{} argument(s), got {n}",
                min,
                if min != max {
                    format!("-{max}")
                } else {
                    String::new()
                }
            )));
        }
    }
    Ok(())
}
