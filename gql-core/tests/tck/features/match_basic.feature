Feature: Match basic patterns (openCypher TCK adapted)

  Background:
    Given an empty graph

  Scenario: Find all nodes
    Given having executed:
      """
      CREATE (n:Person {name: 'Alice'}), (m:Person {name: 'Bob'})
      """
    When executing query:
      """
      MATCH (n:Person) RETURN n.name
      """
    Then the result should contain:
      | n.name  |
      | 'Alice' |
      | 'Bob'   |

  Scenario: Single-hop traversal
    Given having executed:
      """
      CREATE (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'})
      """
    When executing query:
      """
      MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name
      """
    Then the result should contain:
      | a.name  | b.name |
      | 'Alice' | 'Bob'  |

  Scenario: WHERE filtering
    Given having executed:
      """
      CREATE (a:Person {age: 30}), (b:Person {age: 17})
      """
    When executing query:
      """
      MATCH (n:Person) WHERE n.age >= 18 RETURN n.age
      """
    Then the result should contain:
      | n.age |
      | 30    |
