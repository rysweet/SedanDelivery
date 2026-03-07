"""
Integration test for EXPLAIN plan tree structure via the neo4j Python driver.

Verifies that EXPLAIN returns a recursive plan tree with at least 2 levels
of nesting (e.g., ProduceResults > Expand > NodeScan).

Requirements:
    pip install neo4j
    sedandelivery-bolt running on localhost:7687 (or BOLT_URI env var)

Run:
    BOLT_URI=bolt://localhost:7687 pytest tests/integration/test_explain_plan.py -v
"""
import os
import pytest
from neo4j import GraphDatabase

BOLT_URI = os.environ.get("BOLT_URI", "bolt://localhost:7687")


@pytest.fixture(scope="module")
def driver():
    d = GraphDatabase.driver(BOLT_URI, auth=None)
    yield d
    d.close()


def _get_children(node):
    """Extract children from a plan node (dict or Plan object)."""
    if isinstance(node, dict):
        return node.get("children", [])
    return getattr(node, "children", [])


def _get_operator_type(node):
    """Extract operator type from a plan node (dict or Plan object)."""
    if isinstance(node, dict):
        return node.get("operatorType", "")
    return getattr(node, "operator_type", "")


class TestExplainPlan:
    """Test that EXPLAIN returns a recursive plan tree with children."""

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with driver.session() as s:
            self.s = s
            yield

    def test_explain_plan_has_children(self):
        """EXPLAIN should return a plan with non-empty children list."""
        result = self.s.run(
            "EXPLAIN MATCH (n:Person)-[:KNOWS]->(m) RETURN n.name, m.name"
        )
        summary = result.consume()
        plan = summary.plan

        assert plan is not None, "EXPLAIN should return a plan"
        children = _get_children(plan)
        assert len(children) > 0, (
            f"Root plan operator '{_get_operator_type(plan)}' should have "
            f"children, got empty children list"
        )

    def test_explain_plan_at_least_two_levels(self):
        """Plan tree should have at least 2 levels of nesting."""
        result = self.s.run(
            "EXPLAIN MATCH (n:Person)-[:KNOWS]->(m) RETURN n.name, m.name"
        )
        summary = result.consume()
        plan = summary.plan

        assert plan is not None, "EXPLAIN should return a plan"

        # Level 0: root (e.g., ProduceResults)
        children = _get_children(plan)
        assert len(children) > 0, (
            f"Level 0 operator '{_get_operator_type(plan)}' has no children"
        )

        # Level 1: first child (e.g., Expand)
        child = children[0]
        grandchildren = _get_children(child)
        assert len(grandchildren) > 0, (
            f"Level 1 operator '{_get_operator_type(child)}' has no children — "
            f"expected at least 2 levels of nesting"
        )

        # Level 2 exists: grandchild (e.g., NodeScan)
        grandchild = grandchildren[0]
        assert _get_operator_type(grandchild), (
            "Level 2 node should have an operator type"
        )

    def test_explain_plan_operators_are_strings(self):
        """Each plan node should have a non-empty operator_type string."""
        result = self.s.run(
            "EXPLAIN MATCH (n:Person)-[:KNOWS]->(m) RETURN n.name, m.name"
        )
        summary = result.consume()
        plan = summary.plan

        def check_operator_types(node, depth=0):
            op_type = _get_operator_type(node)
            assert isinstance(op_type, str), (
                f"operator_type at depth {depth} should be a string"
            )
            assert len(op_type) > 0, (
                f"operator_type at depth {depth} should not be empty"
            )
            for child in _get_children(node):
                check_operator_types(child, depth + 1)

        check_operator_types(plan)

    def test_explain_plan_tree_structure(self):
        """Verify the full recursive tree structure of the plan."""
        result = self.s.run(
            "EXPLAIN MATCH (n:Person)-[:KNOWS]->(m) RETURN n.name, m.name"
        )
        summary = result.consume()
        plan = summary.plan

        def count_depth(node):
            children = _get_children(node)
            if not children:
                return 1
            return 1 + max(count_depth(c) for c in children)

        depth = count_depth(plan)
        assert depth >= 3, (
            f"Plan tree depth is {depth}, expected at least 3 "
            f"(root > child > grandchild)"
        )
