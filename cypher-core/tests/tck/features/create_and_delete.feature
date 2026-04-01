Feature: CREATE and DELETE (openCypher TCK adapted)

  Scenario: Create node and read it back
    Given an empty graph
    When executing query:
      """
      CREATE (n:Person {name: 'Charlie'})
      """
    Then the side effects should be:
      | +nodes  | 1 |
      | +labels | 1 |
    When executing query:
      """
      MATCH (n:Person {name: 'Charlie'}) RETURN n.name
      """
    Then the result should contain:
      | n.name    |
      | 'Charlie' |
