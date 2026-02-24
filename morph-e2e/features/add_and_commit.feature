# Single user: stage files and commit with program and eval suite.
# Placeholders <prog_hash> and <suite_hash> are filled from earlier captures.

Feature: Add and commit

  Scenario: First commit with program and eval suite
    Given a morph repo
    And a file "src/app.rs" with content "fn main() {}"
    And the identity program and a minimal eval suite exist
    When I run "morph add src/app.rs"
    And the last command succeeded
    When I run "morph add .morph/evals/e.json"
    And I capture the last output as "suite_hash"
    And the last command succeeded
    When I run "morph program create prog.json"
    And I capture the last output as "prog_hash"
    And the last command succeeded
    When I run commit with message "first commit" using captured program and eval suite
    And the last command succeeded
    When I run "morph log HEAD"
    And the last command succeeded
    Then the path ".morph/refs/heads/main" is present
    And the path ".morph/index.json" does not exist
