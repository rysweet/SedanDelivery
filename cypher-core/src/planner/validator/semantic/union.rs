//! UNION validation: column count/name compatibility and uniformity of
//! `UNION` vs `UNION ALL`.

use crate::ast::{Clause, Expr};
use crate::error::GqlError;
use crate::planner::validator::{plan_error_semantic, ValidationContext};

impl ValidationContext {
    pub(in crate::planner::validator) fn validate_unions(
        &self,
        query: &crate::ast::Query,
    ) -> Result<(), GqlError> {
        // Check that all UNION parts are either all UNION or all UNION ALL (no mixing)
        if query.unions.len() >= 2 {
            let first_is_all = query.unions[0].0;
            for (i, (is_all, _)) in query.unions.iter().enumerate().skip(1) {
                if *is_all != first_is_all {
                    return Err(plan_error_semantic(
                        "invalid clause composition: cannot mix UNION and UNION ALL",
                    ));
                }
                let _ = i;
            }
        }

        // Check that all UNION parts have the same number and names of RETURN columns
        let first_return = Self::find_return_columns(&query.clauses);
        for (_, part_clauses) in &query.unions {
            let part_return = Self::find_return_columns(part_clauses);
            if let (Some(ref first), Some(ref part)) = (&first_return, &part_return) {
                if first.len() != part.len() {
                    return Err(plan_error_semantic(
                        "all sub queries in a UNION must have the same column names",
                    ));
                }
                for (f, p) in first.iter().zip(part.iter()) {
                    if f != p {
                        return Err(plan_error_semantic(
                            "all sub queries in a UNION must have the same column names",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn find_return_columns(clauses: &[Clause]) -> Option<Vec<String>> {
        for clause in clauses {
            if let Clause::Return(rc) = clause {
                let names: Vec<String> = rc
                    .items
                    .iter()
                    .map(|item| {
                        if let Some(ref alias) = item.alias {
                            alias.0.clone()
                        } else if let Expr::Variable(ref id) = item.expr {
                            id.0.clone()
                        } else {
                            // Expression without alias
                            String::new()
                        }
                    })
                    .collect();
                return Some(names);
            }
        }
        None
    }
}
