Feature: Spec-first eval-driven workflow (Phase 6c)

  # End-to-end flow that the new prompts encourage:
  #   1. Write the acceptance case (YAML).
  #   2. Register it via `morph eval add-case`.
  #   3. Stage + commit the implementation with metrics, stamping
  #      `--new-cases` so merge plans surface case provenance.
  #   4. `morph eval gaps` reports zero outstanding gaps.
  #   5. `morph eval suite-show` confirms the suite has the case.

  Scenario: Spec → register → implement → commit → zero gaps
    Given a morph repo
    And a file "specs/login.yaml" with content "[{name: alpha}]"
    When I run "morph eval add-case specs/login.yaml"
    Then the last command succeeded
    When I run "morph eval suite-show"
    Then the last command succeeded
    And stdout contains "alpha"
    Given a file "src/feature.rs" with content "pub fn alpha() -> bool { true }"
    When I run "morph add ."
    Then the last command succeeded
    When I commit message "spec-first-impl" with metrics "tests_total=1,tests_passed=1" and new-cases "login:alpha"
    Then the last command succeeded
    When I run "morph eval gaps"
    Then the last command succeeded
    And stdout contains "No behavioral evidence gaps"

  Scenario: Merge plan surfaces case provenance from --new-cases
    Given a morph repo
    And a file "base.txt" with content "v0"
    When I run "morph add base.txt"
    Then the last command succeeded
    When I commit message "base" with metrics "tests_total=1,tests_passed=1" and new-cases "shared"
    Then the last command succeeded
    When I run "morph branch feature"
    Then the last command succeeded
    Given a file "main_only.txt" with content "main"
    When I run "morph add main_only.txt"
    Then the last command succeeded
    When I commit message "main-work" with metrics "tests_total=1,tests_passed=1" and new-cases "alpha,beta"
    Then the last command succeeded
    When I run "morph checkout feature"
    Then the last command succeeded
    Given a file "feature_only.txt" with content "feature"
    When I run "morph add feature_only.txt"
    Then the last command succeeded
    When I commit message "feature-work" with metrics "tests_total=1,tests_passed=1" and new-cases "gamma"
    Then the last command succeeded
    When I run "morph checkout main"
    Then the last command succeeded
    When I run "morph merge-plan feature"
    Then the last command succeeded
    And stdout contains "Case provenance:"
    And stdout contains "main introduces 2 case(s): alpha, beta"
    And stdout contains "feature introduces 1 case(s): gamma"
    And stdout contains "Merged candidate must pass all 3"
