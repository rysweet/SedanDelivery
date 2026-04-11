"""Static contract tests for the embedded-engine audit tranche."""

from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
WORKSPACE_TOML = ROOT / "Cargo.toml"
CI_YML = ROOT / ".github" / "workflows" / "ci.yml"
VALIDATE_SH = ROOT / "scripts" / "validate.sh"
README_MD = ROOT / "README.md"
PRODUCT_SURFACES_MD = ROOT / "docs" / "reference" / "product-surfaces.md"
TUTORIAL_MD = ROOT / "docs" / "tutorials" / "embed-sedandelivery.md"
SEDAN_GQL_LIB = ROOT / "sedan-gql-embedded" / "src" / "lib.rs"
SEDAN_GQL_TOML = ROOT / "sedan-gql-embedded" / "Cargo.toml"
SEDAN_CYPHER_LIB = ROOT / "sedan-cypher-embedded" / "src" / "lib.rs"
SEDAN_CYPHER_TOML = ROOT / "sedan-cypher-embedded" / "Cargo.toml"

EMBEDDED_TEST_COMMAND = (
    "cargo test -p sedan-cypher-embedded -p sedan-gql-embedded "
    "-p sedan-gremlin-embedded"
)
GQL_OUTSIDE_IN_COMMAND = "cargo test -p gql-engine --test outside_in"


def _squash_whitespace(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip()


class TestProductSurfaceInventory:
    def test_workspace_lists_current_main_surface_members(self) -> None:
        text = WORKSPACE_TOML.read_text()
        for member in [
            '"sedan-gql-embedded"',
            '"sedan-cypher-embedded"',
            '"sedan-gremlin-embedded"',
            '"sd2-api"',
            '"sedandelivery-bolt"',
            '"sedandelivery-gremlin"',
            '"sedandelivery-gremlin-server"',
            '"sd2-redis-module"',
            '"gql-cli"',
            '"sedandelivery-fabric-sync"',
            '"gql-engine"',
        ]:
            assert member in text

    def test_product_surface_reference_lists_required_form_factors(self) -> None:
        text = PRODUCT_SURFACES_MD.read_text()
        for surface in [
            "sedan-cypher-embedded",
            "sedan-gql-embedded",
            "sedan-gremlin-embedded",
            "sd2-api",
            "sedandelivery-bolt",
            "sedandelivery-gremlin",
            "sedandelivery-gremlin-server",
            "sd2-redis-module",
            "sd2-valkey-module",
            "gql-cli",
            "sedandelivery-fabric-sync",
        ]:
            assert surface in text

    def test_readme_links_embedded_audit_docs(self) -> None:
        text = README_MD.read_text()
        assert "docs/reference/product-surfaces.md" in text
        assert "docs/reference/embedded-engines.md" in text
        assert "docs/tutorials/embed-sedandelivery.md" in text

    def test_embedded_tutorial_keeps_gql_walkthrough_in_single_code_block(self) -> None:
        text = TUTORIAL_MD.read_text()
        section_match = re.search(
            r"## Step 4: Use `SedanGql` for Native GQL Queries.*?```rust\n(.*?)\n```",
            text,
            re.DOTALL,
        )
        assert section_match, "Step 4 should contain a fenced Rust example"

        section = section_match.group(1)
        assert 'let shared = gql.execute(' in section
        assert 'let unwound = gql.execute(&store, "demo", "FOR x IN [1, 2, 3] RETURN x")?;' in section
        assert (
            '"MANDATORY MATCH (n:Person {name: \'Alice\'}) RETURN n.name"' in section
        ), "tutorial should use a deterministic mandatory-match example"


class TestEmbeddedWiringContracts:
    def test_sedan_gql_embedded_uses_gql_engine_entrypoint(self) -> None:
        lib_text = SEDAN_GQL_LIB.read_text()
        manifest_text = SEDAN_GQL_TOML.read_text()

        assert "gql-engine" in manifest_text, (
            "sedan-gql-embedded must depend on gql-engine so its public API is "
            "locked to the native GQL parser/planner path"
        )
        assert "gql_engine" in lib_text, (
            "sedan-gql-embedded must call into gql-engine from its public API"
        )
        assert "cypher_core::parser::parse_query" not in lib_text
        assert "cypher_core::planner::QueryPlanner" not in lib_text

    def test_sedan_cypher_embedded_wraps_cypher_core_directly(self) -> None:
        lib_text = SEDAN_CYPHER_LIB.read_text()
        manifest_text = SEDAN_CYPHER_TOML.read_text()

        assert "cypher-core" in manifest_text, (
            "sedan-cypher-embedded must depend on cypher-core directly"
        )
        assert "sedan-gql-embedded" not in manifest_text, (
            "sedan-cypher-embedded should not route through sedan-gql-embedded"
        )
        assert "cypher_core" in lib_text, (
            "sedan-cypher-embedded should wire its public API to cypher-core"
        )
        assert "sedan_gql_embedded" not in lib_text


class TestValidationGuardrails:
    def test_ci_runs_embedded_contract_suite(self) -> None:
        text = _squash_whitespace(CI_YML.read_text())
        assert EMBEDDED_TEST_COMMAND in text
        assert GQL_OUTSIDE_IN_COMMAND in text

    def test_local_validate_script_runs_embedded_contract_suite(self) -> None:
        text = _squash_whitespace(VALIDATE_SH.read_text())
        assert EMBEDDED_TEST_COMMAND in text
        assert GQL_OUTSIDE_IN_COMMAND in text
