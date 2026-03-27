Feature: Team CI workflow (Phase 6)

  Scenario: Developer branch is certified in CI
    Given a morph repo
    And a file "code.txt" with content "fn main() {}"
    When I run "morph add code.txt"
    Then the last command succeeded
    When I run "morph commit -m initial-commit"
    Then the last command succeeded
    When I capture the last output as "commit_hash"
    When I create a JSON file "metrics.json" with metrics "tests_passed=42,tests_total=42,pass_rate=1.0"
    When I run "morph certify --metrics-file metrics.json"
    Then the last command succeeded
    And stdout contains "PASS"
    When I run "morph gate"
    Then the last command succeeded
    And stdout contains "PASS"

  Scenario: Candidate is blocked by policy
    Given a morph repo
    And a file "code.txt" with content "fn main() {}"
    When I create a policy file "policy.json" with required "tests_passed,coverage_pct" and thresholds "tests_passed=1.0,coverage_pct=0.8"
    When I run "morph policy set policy.json"
    Then the last command succeeded
    When I run "morph add code.txt"
    Then the last command succeeded
    When I run "morph commit -m candidate-commit"
    Then the last command succeeded
    When I create a JSON file "metrics.json" with metrics "tests_passed=42"
    When I run "morph certify --metrics-file metrics.json"
    Then the last command failed
    And stderr contains "FAIL"
    And stderr contains "coverage_pct"
    When I run "morph gate"
    Then the last command failed
    And stderr contains "FAIL"

  Scenario: Git-style collaboration workflow with Morph sidecar
    Given a morph repo
    And a file "src/lib.rs" with content "pub fn add(a: i32, b: i32) -> i32 { a + b }"
    When I run "morph add src/lib.rs"
    Then the last command succeeded
    When I run "morph commit -m main-baseline"
    Then the last command succeeded
    When I capture the last output as "main_commit"
    When I run "morph branch feature-auth"
    Then the last command succeeded
    When I run "morph checkout feature-auth"
    Then the last command succeeded
    Given a file "src/auth.rs" with content "pub fn login() -> bool { true }"
    When I run "morph add src/auth.rs"
    Then the last command succeeded
    When I run "morph commit -m add-auth-module"
    Then the last command succeeded
    When I capture the last output as "feature_commit"
    When I create a JSON file "ci-metrics.json" with metrics "tests_passed=10,tests_total=10"
    When I run "morph certify --metrics-file ci-metrics.json --runner github-actions"
    Then the last command succeeded
    And stdout contains "PASS"
    When I run "morph gate"
    Then the last command succeeded
    And stdout contains "PASS"
    When I run "morph gate --json"
    Then the last command succeeded
    And stdout contains "passed"
    And stdout contains "true"
