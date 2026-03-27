Feature: Evidence-backed commits (Phase 2)

  Scenario: Session to evidence-backed commit
    Given a morph repo
    And a file "src/app.rs" with content "fn main() {}"
    When I run record-session with prompt "fix the bug" and response "done fixing"
    And the last command succeeded
    And I capture the last output as "run_hash"
    When I run "morph add src/app.rs"
    And the last command succeeded
    When I commit with from-run "<run_hash>" and message "evidence-commit"
    And the last command succeeded
    And I capture the last output as "commit_hash"
    When I run "morph show <commit_hash>"
    And the last command succeeded
    Then stdout contains "evidence_refs"
    And stdout contains "env_constraints"
    And stdout contains "contributors"
    And stdout contains "<run_hash>"

  Scenario: Multi-contributor run to commit
    Given a morph repo
    And a file "src/app.rs" with content "fn main() {}"
    When I run record-session with prompt "build feature" and response "feature built"
    And the last command succeeded
    And I capture the last output as "run_hash"
    When I run "morph add src/app.rs"
    And the last command succeeded
    When I commit with from-run "<run_hash>" and message "multi-contributor-commit"
    And the last command succeeded
    And I capture the last output as "commit_hash"
    When I run "morph show <commit_hash>"
    And the last command succeeded
    Then stdout contains "contributors"
    And stdout contains "primary"
    And stdout contains "evidence_refs"
    And stdout contains "<run_hash>"
