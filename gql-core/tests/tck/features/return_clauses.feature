Feature: RETURN clause (openCypher TCK adapted)

  Scenario: Return distinct
    Given an empty graph
    And having executed:
      """
      CREATE (:Dept {name: 'Eng'}), (:Dept {name: 'Eng'}), (:Dept {name: 'HR'})
      """
    When executing query:
      """
      MATCH (d:Dept) RETURN DISTINCT d.name
      """
    Then the result should contain:
      | d.name |
      | 'Eng'  |
      | 'HR'   |

  Scenario: Return with ORDER BY
    Given an empty graph
    And having executed:
      """
      CREATE (a:N {n: 3}), (b:N {n: 1}), (c:N {n: 2})
      """
    When executing query:
      """
      MATCH (n:N) RETURN n.n ORDER BY n.n
      """
    Then the result should be, in order:
      | n.n |
      | 1   |
      | 2   |
      | 3   |
