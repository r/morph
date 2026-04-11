# Human-readable E2E spec: you can read this to understand what we're testing.
# The Cucumber framework runs these steps; step definitions are in tests/cucumber.rs.

Feature: Morph init and status

  Scenario: Status shows new file after init
    Given a morph repo
    And a file "hello.txt" with content "world"
    When I run "morph status"
    Then stdout contains "hello.txt"
    And stdout contains "new file"
    And the path ".morph" exists as a directory
    And the path ".morph/objects" exists as a directory
    And the path ".morph/refs/heads" exists as a directory

  Scenario: Empty repo has no files to track
    Given a morph repo
    When I run "morph status"
    Then stdout contains "nothing to commit"
