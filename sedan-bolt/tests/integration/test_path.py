"""
Integration tests for Bolt path serialization via the neo4j Python driver.

Verifies that sedan-bolt correctly encodes Path structures (tag 0x50) with:
- Unique Node structures (tag 0x4E: id, labels, properties)
- UnboundRelationship structures (tag 0x72: id, type, properties)
- Correct index sequence (rel_index: 1-based signed, node_index: 0-based)

Requirements:
    pip install neo4j
    sedan-bolt running on localhost:17687 (or BOLT_URI env var)

Run:
    BOLT_URI=bolt://localhost:17687 pytest tests/integration/test_path.py -v
"""
import os
import pytest
from neo4j import GraphDatabase
from neo4j.graph import Path, Node, Relationship

BOLT_URI = os.environ.get("BOLT_URI", "bolt://localhost:17687")


@pytest.fixture(scope="module")
def driver():
    d = GraphDatabase.driver(BOLT_URI, auth=None)
    yield d
    d.close()


class TestPathSerialization:
    """Test that paths returned from sedan-bolt are correctly deserialized."""

    DB = "test_path"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with driver.session(database=self.DB) as s:
            self.s = s
            yield

    def test_simple_path_two_nodes_one_edge(self):
        """A->B path: 2 nodes, 1 relationship."""
        self.s.run(
            "CREATE (a:SP1 {name:'A'})-[:R {w:1}]->(b:SP2 {name:'B'})"
        ).consume()

        result = self.s.run(
            "MATCH p=(a:SP1)-[:R]->(b:SP2) RETURN p"
        )
        records = list(result)
        assert len(records) >= 1, "Expected at least one path result"

        path = records[0]["p"]
        assert isinstance(path, Path), f"Expected Path, got {type(path)}"

        nodes = list(path.nodes)
        assert len(nodes) == 2, f"Expected 2 nodes, got {len(nodes)}"
        assert "SP1" in nodes[0].labels
        assert nodes[0]["name"] == "A"
        assert "SP2" in nodes[1].labels
        assert nodes[1]["name"] == "B"

        rels = list(path.relationships)
        assert len(rels) == 1, f"Expected 1 relationship, got {len(rels)}"
        assert rels[0].type == "R"
        assert rels[0]["w"] == 1

    def test_path_three_nodes_two_edges(self):
        """A->B->C path as requested: 3 nodes, 2 relationships with properties."""
        self.s.run(
            "CREATE (a:P1 {name:'A'})-[:R {w:1}]->(b:P2 {name:'B'})"
            "-[:R {w:2}]->(c:P3 {name:'C'})"
        ).consume()

        result = self.s.run(
            "MATCH p=(a:P1)-[*]->(c:P3) RETURN p"
        )
        records = list(result)
        assert len(records) >= 1, "Expected at least one path result"

        path = records[0]["p"]
        assert isinstance(path, Path), f"Expected Path, got {type(path)}"

        nodes = list(path.nodes)
        assert len(nodes) == 3, f"Expected 3 nodes, got {len(nodes)}"
        assert "P1" in nodes[0].labels
        assert nodes[0]["name"] == "A"
        assert "P2" in nodes[1].labels
        assert nodes[1]["name"] == "B"
        assert "P3" in nodes[2].labels
        assert nodes[2]["name"] == "C"

        rels = list(path.relationships)
        assert len(rels) == 2, f"Expected 2 relationships, got {len(rels)}"
        assert rels[0].type == "R"
        assert rels[0]["w"] == 1
        assert rels[1].type == "R"
        assert rels[1]["w"] == 2

    def test_path_start_and_end_nodes(self):
        """Verify start_node and end_node of the path."""
        self.s.run(
            "CREATE (a:SE1 {name:'start'})-[:LINK]->(b:SE2 {name:'end'})"
        ).consume()

        records = list(self.s.run(
            "MATCH p=(a:SE1)-[:LINK]->(b:SE2) RETURN p"
        ))
        path = records[0]["p"]
        assert path.start_node["name"] == "start"
        assert path.end_node["name"] == "end"

    def test_path_length(self):
        """Verify len(path) returns number of relationships."""
        self.s.run(
            "CREATE (:PL1 {n:1})-[:E]->(:PL2 {n:2})-[:E]->(:PL3 {n:3})"
        ).consume()

        records = list(self.s.run(
            "MATCH p=(:PL1)-[*]->(:PL3) RETURN p"
        ))
        path = records[0]["p"]
        assert len(path) == 2, f"Expected path length 2, got {len(path)}"

    def test_single_node_path(self):
        """A path with just one node and no relationships."""
        self.s.run("CREATE (:Solo {name:'alone'})").consume()

        records = list(self.s.run(
            "MATCH p=(:Solo) RETURN p"
        ))
        assert len(records) >= 1
        path = records[0]["p"]
        assert isinstance(path, Path)
        nodes = list(path.nodes)
        assert len(nodes) == 1
        assert nodes[0]["name"] == "alone"
        rels = list(path.relationships)
        assert len(rels) == 0

    def test_path_node_properties_preserved(self):
        """Ensure node properties are not lost in path encoding."""
        self.s.run(
            "CREATE (a:NP1 {x:1, y:'hello', z:true})"
            "-[:EDGE {weight:3.14}]->"
            "(b:NP2 {x:2, y:'world', z:false})"
        ).consume()

        records = list(self.s.run(
            "MATCH p=(:NP1)-[:EDGE]->(:NP2) RETURN p"
        ))
        path = records[0]["p"]
        nodes = list(path.nodes)

        assert nodes[0]["x"] == 1
        assert nodes[0]["y"] == "hello"
        assert nodes[0]["z"] is True

        assert nodes[1]["x"] == 2
        assert nodes[1]["y"] == "world"
        assert nodes[1]["z"] is False

        rels = list(path.relationships)
        assert abs(rels[0]["weight"] - 3.14) < 0.001

    def test_path_relationship_types_diverse(self):
        """Path with different relationship types."""
        self.s.run(
            "CREATE (a:DT1 {name:'A'})-[:KNOWS]->(b:DT2 {name:'B'})"
            "-[:LIKES]->(c:DT3 {name:'C'})"
        ).consume()

        records = list(self.s.run(
            "MATCH p=(:DT1)-[*]->(:DT3) RETURN p"
        ))
        path = records[0]["p"]
        rels = list(path.relationships)
        assert len(rels) == 2
        rel_types = {r.type for r in rels}
        assert "KNOWS" in rel_types
        assert "LIKES" in rel_types

    def test_path_echo_with_individual_entities(self):
        """Mirrors testkit test_should_echo_path: return path alongside
        individual nodes and relationships, verify they match.

        The testkit version uses CREATE p=(a)-[ab:X]->(b)-[bc:X]->(c)
        which requires path variable binding in CREATE. We work around
        this by using separate CREATE and MATCH statements.
        """
        self.s.run(
            "CREATE (:Echo1)-[:X]->(:Echo2)-[:X]->(:Echo3)"
        ).consume()

        # Return both individual entities and the full path
        records = list(self.s.run(
            "MATCH (a:Echo1)-[ab:X]->(b:Echo2)-[bc:X]->(c:Echo3) "
            "MATCH p=(a)-[:X]->(b)-[:X]->(c) "
            "RETURN a, b, c, ab, bc, p"
        ))
        assert len(records) >= 1

        r = records[0]
        a, b, c = r["a"], r["b"], r["c"]
        ab, bc = r["ab"], r["bc"]
        p = r["p"]

        # All types correct
        assert isinstance(a, Node)
        assert isinstance(b, Node)
        assert isinstance(c, Node)
        assert isinstance(ab, Relationship)
        assert isinstance(bc, Relationship)
        assert isinstance(p, Path)

        # Node IDs all distinct
        assert a.element_id != b.element_id
        assert a.element_id != c.element_id
        assert b.element_id != c.element_id

        # Relationship endpoints match
        assert ab.start_node.element_id == a.element_id
        assert ab.end_node.element_id == b.element_id
        assert bc.start_node.element_id == b.element_id
        assert bc.end_node.element_id == c.element_id

        # Path structure matches
        path_nodes = list(p.nodes)
        assert len(path_nodes) == 3
        assert path_nodes[0].element_id == a.element_id
        assert path_nodes[1].element_id == b.element_id
        assert path_nodes[2].element_id == c.element_id

        path_rels = list(p.relationships)
        assert len(path_rels) == 2
        assert path_rels[0].element_id == ab.element_id
        assert path_rels[1].element_id == bc.element_id

        # Relationship type preserved
        assert path_rels[0].type == "X"
        assert path_rels[1].type == "X"

    def test_path_rel_start_end_match_path_order(self):
        """Verify that relationship start/end node IDs in the path match
        the traversal order defined by the path's node sequence."""
        self.s.run(
            "CREATE (:RO1 {name:'first'})-[:STEP {n:1}]->"
            "(:RO2 {name:'second'})-[:STEP {n:2}]->"
            "(:RO3 {name:'third'})"
        ).consume()

        records = list(self.s.run(
            "MATCH p=(:RO1)-[*]->(:RO3) RETURN p"
        ))
        path = records[0]["p"]
        nodes = list(path.nodes)
        rels = list(path.relationships)

        assert len(nodes) == 3
        assert len(rels) == 2

        # Each rel's start/end should match consecutive path nodes
        assert rels[0].start_node.element_id == nodes[0].element_id
        assert rels[0].end_node.element_id == nodes[1].element_id
        assert rels[1].start_node.element_id == nodes[1].element_id
        assert rels[1].end_node.element_id == nodes[2].element_id

        # Properties preserved in order
        assert rels[0]["n"] == 1
        assert rels[1]["n"] == 2
