"""Regression tests for Fabric issue 1211 fixes."""

from __future__ import annotations

import importlib.util
import re
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
VALIDATOR = ROOT / "scripts" / "validate-fabric-e2e.py"
PROVISION_FABRIC_SH = ROOT / "scripts" / "provision-fabric.sh"
E2E_FABRIC_YML = ROOT / ".github" / "workflows" / "e2e-fabric.yml"
README_MD = ROOT / "README.md"
FABRIC_TUTORIAL_MD = ROOT / "docs" / "FABRIC_TUTORIAL.md"
SECRETS_MD = ROOT / "docs" / "secrets.md"
INDEX_MD = ROOT / "docs" / "index.md"
AUTO_SCALING_MD = ROOT / "docs" / "auto-scaling.md"
ARCHITECTURE_MD = ROOT / "docs" / "architecture.md"

CANONICAL_SANITIZER = (
    "sed 's/[^a-zA-Z0-9-]/-/g' | tr '[:upper:]' '[:lower:]' | "
    "sed 's/--*/-/g' | cut -c1-20 | sed 's/^-*//;s/-*$//'"
)
LEGACY_SANITIZER = (
    "sed 's/[^a-zA-Z0-9-]/-/g' | tr '[:upper:]' '[:lower:]' | "
    "cut -c1-20 | sed 's/^-*//;s/-*$//'"
)


def _load_validator_module():
    module_name = "validate_fabric_e2e_issue_1211"
    spec = importlib.util.spec_from_file_location(module_name, VALIDATOR)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module_name, module


@pytest.fixture()
def validator_module():
    module_name, module = _load_validator_module()
    try:
        yield module
    finally:
        sys.modules.pop(module_name, None)


def _squash_whitespace(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip()


def _point_snapshot(value):
    return {"point_val": value}


class TestWorkflowEnvironmentSanitizationParity:
    def test_e2e_workflow_matches_deploy_sanitizer_shape(self) -> None:
        text = _squash_whitespace(E2E_FABRIC_YML.read_text())
        assert CANONICAL_SANITIZER in text
        assert LEGACY_SANITIZER not in text

    def test_e2e_workflow_reads_sync_container_app(self) -> None:
        text = E2E_FABRIC_YML.read_text()
        assert '--name "sd2-${ENV_NAME}-sync"' in text
        assert '--name "sd2-${ENV_NAME}-fabric-sync"' not in text


class TestOAuthTokenEncoding:
    def test_provision_script_uses_urlencoded_token_fields(self) -> None:
        text = PROVISION_FABRIC_SH.read_text()
        assert '--data-urlencode "grant_type=client_credentials"' in text
        assert '--data-urlencode "client_id=${CLIENT_ID}"' in text
        assert '--data-urlencode "client_secret=${CLIENT_SECRET}"' in text
        assert '--data-urlencode "scope=https://api.fabric.microsoft.com/.default"' in text
        assert '-d "grant_type=client_credentials&client_id=' not in text


class TestPointRoundTripValidation:
    @pytest.mark.parametrize(
        "value",
        [
            {"x": 1.0, "y": 2.0},
            {"longitude": 1.0, "latitude": 2.0},
            "POINT(1.0 2.0)",
        ],
    )
    def test_validate_property_point_accepts_supported_shapes(
        self,
        monkeypatch,
        validator_module,
        value,
    ) -> None:
        monkeypatch.setattr(
            validator_module,
            "fabric_query",
            lambda *_args, **_kwargs: {"results": [{"headers": ["v"], "data": [[value]]}]},
        )

        result = validator_module.validate_property_point("token", "https://fabric.example")

        assert result.passed, result.detail

    @pytest.mark.parametrize(
        ("value", "expected_detail"),
        [
            ({"x": 1.0}, "point value must include both x and y"),
            ({"longitude": 1.0, "latitude": "north"}, "latitude must be numeric"),
            ({"x": 1.0, "y": 99.0}, "y=99.0 (expected 2.0)"),
            ("POINT(1.0)", "point value is not point-shaped"),
            ({"x": float("nan"), "y": 2.0}, "x must be finite"),
        ],
    )
    def test_validate_property_point_rejects_lossy_or_malformed_values(
        self,
        monkeypatch,
        validator_module,
        value,
        expected_detail,
    ) -> None:
        monkeypatch.setattr(
            validator_module,
            "fabric_query",
            lambda *_args, **_kwargs: {"results": [{"headers": ["v"], "data": [[value]]}]},
        )

        result = validator_module.validate_property_point("token", "https://fabric.example")

        assert not result.passed
        assert expected_detail in result.detail


class TestFabricContractDocumentation:
    @pytest.mark.parametrize(
        ("path", "expected", "unexpected"),
        [
            (README_MD, "sd2-{env}-sync", "sd2-{env}-fabric-sync"),
            (FABRIC_TUTORIAL_MD, "sd2-{env}-sync", "sd2-{env}-fabric-sync"),
            (SECRETS_MD, "sd2-{env}-sync", "sd2-{env}-fabric-sync"),
            (INDEX_MD, "sd2-env-sync", "sd2-env-fabric-sync"),
            (AUTO_SCALING_MD, "sd2-main-sync", "sd2-main-fabric-sync"),
            (ARCHITECTURE_MD, "sd2-main-sync", "sd2-main-fabric-sync"),
        ],
    )
    def test_docs_use_live_sync_container_app_names(
        self,
        path: Path,
        expected: str,
        unexpected: str,
    ) -> None:
        text = path.read_text()
        assert expected in text
        assert unexpected not in text

    def test_tutorial_uses_graphmodel_rest_contract(self) -> None:
        text = FABRIC_TUTORIAL_MD.read_text()
        assert "GraphModels/{graphmodel-id}/executeQuery?preview=true" in text
        assert "items?type=GraphModel" in text
        assert "graphDatabases/{gdb-id}" not in text
        assert "items?type=GraphQLApi" not in text
