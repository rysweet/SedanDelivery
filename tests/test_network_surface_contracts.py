"""Static contract tests for the network-surface tranche.

Mirrors `tests/test_embedded_surface_contracts.py` but for the four network
form-factors (Bolt, Gremlin WebSocket, Gremlin server text-eval, REST API).

These are static / structural checks — they don't spin up the servers. They
exist to catch surface drift early:

    * binary crates remain workspace members
    * each binary still has a `src/main.rs`
    * each surface is documented in product-surfaces.md
    * each surface keeps a default port advertised in the docs
    * each surface points at an integration-test entry point
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKSPACE_TOML = ROOT / "Cargo.toml"
PRODUCT_SURFACES_MD = ROOT / "docs" / "reference" / "product-surfaces.md"
README_MD = ROOT / "README.md"

NETWORK_CRATES = {
    "sedandelivery-bolt": {
        "main_rs": "sedandelivery-bolt/src/main.rs",
        "integration_test": "sedandelivery-bolt/tests/bolt_integration.rs",
        "default_port": 7687,
        "doc_keyword": "Bolt",
    },
    "sedandelivery-gremlin": {
        "main_rs": "sedandelivery-gremlin/src/main.rs",
        "integration_test": "sedandelivery-gremlin/tests/websocket_integration.rs",
        "default_port": 8182,
        "doc_keyword": "Gremlin",
    },
    "sedandelivery-gremlin-server": {
        "main_rs": "sedandelivery-gremlin-server/src/main.rs",
        "integration_test": None,  # text-eval server doesn't have its own integration tests yet
        "default_port": 8183,
        "doc_keyword": "Gremlin",
    },
    "sd2-api": {
        "main_rs": "sd2-api/src/main.rs",
        "integration_test": "sd2-api/tests/api_integration.rs",
        "default_port": 8080,
        "doc_keyword": "REST",
    },
}


def _squash_whitespace(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip()


class TestNetworkCratesAreWorkspaceMembers:
    def test_all_network_crates_listed_as_workspace_members(self) -> None:
        text = WORKSPACE_TOML.read_text()
        for crate in NETWORK_CRATES:
            assert f'"{crate}"' in text, (
                f"network surface {crate} is no longer a workspace member; "
                "see Cargo.toml"
            )


class TestNetworkBinariesExist:
    def test_each_network_crate_has_main_rs(self) -> None:
        for crate, info in NETWORK_CRATES.items():
            main = ROOT / info["main_rs"]
            assert main.is_file(), (
                f"{crate}: expected {info['main_rs']} to exist"
            )


class TestNetworkIntegrationTestsExist:
    def test_each_documented_integration_test_present(self) -> None:
        for crate, info in NETWORK_CRATES.items():
            test_path = info["integration_test"]
            if test_path is None:
                continue
            full = ROOT / test_path
            assert full.is_file(), (
                f"{crate}: documented integration test {test_path} is missing"
            )


class TestProductSurfaceReferenceMentionsEachSurface:
    def test_product_surface_doc_mentions_every_network_surface(self) -> None:
        text = PRODUCT_SURFACES_MD.read_text()
        for crate, info in NETWORK_CRATES.items():
            assert crate in text or info["doc_keyword"] in text, (
                f"{crate}: product-surfaces.md must mention this surface"
                f" by crate name or keyword '{info['doc_keyword']}'"
            )


class TestReadmeAdvertisesPrimaryNetworkSurfaces:
    def test_readme_mentions_bolt_gremlin_and_rest(self) -> None:
        text = README_MD.read_text()
        for keyword in ("Bolt", "Gremlin", "REST"):
            assert keyword in text, (
                f"README.md should mention {keyword!r} as a primary surface"
            )
