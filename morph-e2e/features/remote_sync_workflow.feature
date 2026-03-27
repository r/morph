Feature: Remote sync workflow (Phase 5)

  Scenario: Two repos synchronize through a named remote
    Given a morph repo
    Given a second morph repo at "remote"
    And a file "hello.txt" with content "hello world"
    When I run "morph add hello.txt"
    And the last command succeeded
    When I run "morph commit -m first-commit"
    And the last command succeeded
    And I capture the last output as "commit_hash"
    When I run "morph remote add origin <remote>"
    And the last command succeeded
    When I run "morph push origin main"
    And the last command succeeded
    Then stdout contains "Pushed"
    When I run "morph log" in directory "remote"
    And the last command succeeded
    Then stdout contains "first-commit"

  Scenario: Fast-forward pull updates local state
    Given a morph repo
    Given a second morph repo at "remote"
    And a file "data.txt" in directory "remote" with content "remote data"
    When I run "morph add data.txt" in directory "remote"
    And the last command succeeded
    When I run "morph commit -m remote-commit" in directory "remote"
    And the last command succeeded
    When I run "morph remote add origin <remote>"
    And the last command succeeded
    When I run "morph pull origin main"
    And the last command succeeded
    Then stdout contains "Updated"
    When I run "morph log"
    And the last command succeeded
    Then stdout contains "remote-commit"

  Scenario: Non-fast-forward push is rejected
    Given a morph repo
    Given a second morph repo at "remote"
    And a file "local.txt" with content "local content"
    When I run "morph add local.txt"
    And the last command succeeded
    When I run "morph commit -m local-commit"
    And the last command succeeded
    And a file "remote.txt" in directory "remote" with content "remote content"
    When I run "morph add remote.txt" in directory "remote"
    And the last command succeeded
    When I run "morph commit -m remote-commit" in directory "remote"
    And the last command succeeded
    When I run "morph remote add origin <remote>"
    And the last command succeeded
    When I run "morph push origin main"
    Then the last command failed
    And stderr contains "non-fast-forward"

  Scenario: Evidence-backed history survives sync
    Given a morph repo
    Given a second morph repo at "remote"
    And a file "code.txt" with content "fn main() {}"
    When I run record-session with prompt "fix the bug" and response "done fixing"
    And the last command succeeded
    And I capture the last output as "run_hash"
    When I run "morph add code.txt"
    And the last command succeeded
    When I commit with from-run "<run_hash>" and message "evidence-commit"
    And the last command succeeded
    And I capture the last output as "commit_hash"
    When I run "morph remote add origin <remote>"
    And the last command succeeded
    When I run "morph push origin main"
    And the last command succeeded
    When I run "morph show <commit_hash>" in directory "remote"
    And the last command succeeded
    Then stdout contains "evidence_refs"
    And stdout contains "<run_hash>"
