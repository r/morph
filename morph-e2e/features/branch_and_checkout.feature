Feature: Branch and checkout workflow

  Scenario: Create branch, commit on it, switch back to main
    Given a morph repo
    And a file "main_file.txt" with content "on main"
    When I run "morph add main_file.txt"
    And the last command succeeded
    When I run "morph commit -m initial"
    And the last command succeeded
    When I run "morph branch feature"
    And the last command succeeded
    When I run "morph checkout feature"
    And the last command succeeded
    Then stdout contains "Switched to branch feature"
    Given a file "feature_file.txt" with content "on feature"
    When I run "morph add feature_file.txt"
    And the last command succeeded
    When I run "morph commit -m feature-work"
    And the last command succeeded
    When I run "morph checkout main"
    And the last command succeeded
    Then stdout contains "Switched to branch main"
    And the file "main_file.txt" has content "on main"
    And the path "feature_file.txt" does not exist

  Scenario: Checkout restores working tree from commit
    Given a morph repo
    And a file "a.txt" with content "aaa"
    When I run "morph add a.txt"
    And the last command succeeded
    When I run "morph commit -m main-commit"
    And the last command succeeded
    When I run "morph branch feature"
    And the last command succeeded
    When I run "morph checkout feature"
    And the last command succeeded
    Given a file "b.txt" with content "bbb"
    When I run "morph add b.txt"
    And the last command succeeded
    When I run "morph commit -m feature-commit"
    And the last command succeeded
    When I run "morph checkout main"
    And the last command succeeded
    Then the path "a.txt" is present
    And the path "b.txt" does not exist
