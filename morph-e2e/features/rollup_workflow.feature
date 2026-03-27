Feature: Rollup squashes commits

  Scenario: Rollup three commits into one
    Given a morph repo
    And a file "a.txt" with content "aaa"
    When I run "morph add a.txt"
    And the last command succeeded
    When I run "morph commit -m first"
    And the last command succeeded
    And I capture the last output as "base_hash"
    Given a file "b.txt" with content "bbb"
    When I run "morph add b.txt"
    And the last command succeeded
    When I run "morph commit -m second"
    And the last command succeeded
    Given a file "c.txt" with content "ccc"
    When I run "morph add c.txt"
    And the last command succeeded
    When I run "morph commit -m third"
    And the last command succeeded
    When I run "morph rollup <base_hash> HEAD -m squashed"
    And the last command succeeded
    When I run "morph log"
    Then the last command succeeded
    And stdout contains "squashed"
