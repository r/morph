Feature: Behavioral merge workflow (Phase 4)

  Scenario: User previews and executes a successful merge
    Given a morph repo
    And the identity pipeline and a minimal eval suite exist
    When I run "morph pipeline create prog.json"
    And the last command succeeded
    And I capture the last output as "prog_hash"
    When I run "morph add .morph/evals/e.json"
    And the last command succeeded
    And I capture the last output as "suite_hash"
    Given a file "a.txt" with content "aaa"
    When I run "morph add a.txt"
    And the last command succeeded
    When I commit with message "main-commit" pipeline "<prog_hash>" suite "<suite_hash>" and metrics {"acc": 0.9}
    And the last command succeeded
    When I run "morph branch feature"
    And the last command succeeded
    When I run "morph checkout feature"
    And the last command succeeded
    Given a file "b.txt" with content "bbb"
    When I run "morph add b.txt"
    And the last command succeeded
    When I commit with message "feature-commit" pipeline "<prog_hash>" suite "<suite_hash>" and metrics {"acc": 0.85}
    And the last command succeeded
    When I run "morph checkout main"
    And the last command succeeded
    When I run "morph merge-plan feature"
    Then the last command succeeded
    And stdout contains "Merge plan"
    And stdout contains "Reference bar"
    And stdout contains "acc"
    When I merge "feature" with message "merged" pipeline "<prog_hash>" and metrics {"acc": 0.92}
    And the last command succeeded
    When I run "morph log"
    Then the last command succeeded
    And stdout contains "merged"

  Scenario: Merge is rejected with a clear explanation
    Given a morph repo
    And the identity pipeline and a minimal eval suite exist
    When I run "morph pipeline create prog.json"
    And the last command succeeded
    And I capture the last output as "prog_hash"
    When I run "morph add .morph/evals/e.json"
    And the last command succeeded
    And I capture the last output as "suite_hash"
    Given a file "a.txt" with content "aaa"
    When I run "morph add a.txt"
    And the last command succeeded
    When I commit with message "main-commit" pipeline "<prog_hash>" suite "<suite_hash>" and metrics {"acc": 0.9}
    And the last command succeeded
    When I run "morph branch feature"
    And the last command succeeded
    When I run "morph checkout feature"
    And the last command succeeded
    Given a file "b.txt" with content "bbb"
    When I run "morph add b.txt"
    And the last command succeeded
    When I commit with message "feature-commit" pipeline "<prog_hash>" suite "<suite_hash>" and metrics {"acc": 0.85}
    And the last command succeeded
    When I run "morph checkout main"
    And the last command succeeded
    When I merge "feature" with message "bad-merge" pipeline "<prog_hash>" and metrics {"acc": 0.87}
    Then the last command failed
    And stderr contains "rejected"
    And stderr contains "acc"

  Scenario: Merge with metric retirement
    Given a morph repo
    And an eval suite with acc and old_metric
    When I run "morph pipeline create prog.json"
    And the last command succeeded
    And I capture the last output as "prog_hash"
    When I run "morph add .morph/evals/e.json"
    And the last command succeeded
    And I capture the last output as "suite_hash"
    Given a file "a.txt" with content "aaa"
    When I run "morph add a.txt"
    And the last command succeeded
    When I commit with message "main-commit" pipeline "<prog_hash>" suite "<suite_hash>" and metrics {"acc": 0.9, "old_metric": 0.8}
    And the last command succeeded
    When I run "morph branch feature"
    And the last command succeeded
    When I run "morph checkout feature"
    And the last command succeeded
    Given a file "b.txt" with content "bbb"
    When I run "morph add b.txt"
    And the last command succeeded
    When I commit with message "feature-commit" pipeline "<prog_hash>" suite "<suite_hash>" and metrics {"acc": 0.85, "old_metric": 0.7}
    And the last command succeeded
    When I run "morph checkout main"
    And the last command succeeded
    When I run "morph merge-plan feature"
    Then the last command succeeded
    And stdout contains "old_metric"
    When I merge "feature" with message "retired-merge" pipeline "<prog_hash>" metrics {"acc": 0.92} and retire "old_metric"
    And the last command succeeded
    When I run "morph log"
    Then the last command succeeded
    And stdout contains "retired-merge"
