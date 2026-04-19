//! Integration test entry point for the differential parity suite.
//!
//! Each [`ParityCase`] in [`differential::cases::CASES`] runs through all
//! three engines on the shared fixture and is asserted equivalent (or
//! tracked as a documented divergence).

use differential::{build_fixture, cases::CASES, run_case, DivergenceMode};

#[test]
fn parity_suite() {
    let store = build_fixture();
    let mut strict_failures: Vec<String> = Vec::new();
    let mut documented_divergences: Vec<String> = Vec::new();
    let mut strict_total = 0usize;
    let mut documented_total = 0usize;

    for case in CASES {
        let outcome = run_case(&store, case);

        match outcome.mode {
            DivergenceMode::Strict => {
                strict_total += 1;
                if !outcome.agrees() {
                    strict_failures.push(outcome.diagnostic());
                } else {
                    println!(
                        "[strict ✓] {} ({} rows)",
                        outcome.name,
                        outcome.cypher.len()
                    );
                }
            }
            DivergenceMode::Documented(reason) => {
                documented_total += 1;
                if outcome.agrees() {
                    println!(
                        "[documented ✓ unexpectedly converged] {} — engines now agree; \
                         consider promoting to Strict. Reason was: {}",
                        outcome.name, reason
                    );
                } else {
                    documented_divergences.push(format!(
                        "[documented] {} — {}\n{}",
                        outcome.name,
                        reason,
                        outcome.diagnostic()
                    ));
                }
            }
        }
    }

    println!(
        "\n=== differential parity summary ===\n\
         strict cases:     {strict_total} ({} failures)\n\
         documented cases: {documented_total} ({} divergences observed)\n",
        strict_failures.len(),
        documented_divergences.len()
    );

    for d in &documented_divergences {
        println!("{d}");
    }

    if !strict_failures.is_empty() {
        for f in &strict_failures {
            eprintln!("{f}");
        }
        panic!(
            "{} strict parity case(s) failed — see diagnostics above. \
             If a divergence is intentional, move the case to DivergenceMode::Documented \
             and document the rationale in docs/DIFFERENTIAL_TESTING.md.",
            strict_failures.len()
        );
    }
}
