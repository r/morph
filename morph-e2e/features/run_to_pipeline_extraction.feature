Feature: Run to Pipeline extraction (Phase 3)

  Scenario: Session to inspectable reusable pipeline
    Given a morph repo
    And a file "src/app.rs" with content "fn main() {}"
    When I run record-session with prompt "fix the bug" and response "done fixing"
    And the last command succeeded
    And I capture the last output as "run_hash"
    When I run "morph pipeline extract --from-run <run_hash>"
    And the last command succeeded
    And I capture the last output as "pipeline_hash"
    When I run "morph pipeline show <pipeline_hash>"
    And the last command succeeded
    Then stdout contains "prompt_call"
    And stdout contains "review"
    And stdout contains "derived_from_run"
    And stdout contains "derived_from_trace"
    And stdout contains "derived_from_event"
    And stdout contains "extracted"
    And stdout contains "generate"
    And stdout contains "<run_hash>"
    When I run "morph add src/app.rs"
    And the last command succeeded
    When I run "morph commit -m pipeline-commit --pipeline <pipeline_hash> --json"
    And the last command succeeded
    And I capture the last output as "commit_hash"
    When I run "morph show <commit_hash>"
    And the last command succeeded
    Then stdout contains "<pipeline_hash>"

  Scenario: Extraction fails clearly for missing Run
    Given a morph repo
    When I run "morph pipeline extract --from-run aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    Then the last command failed
