# Phase 2: multiple agents acting on the same repo at the same time.
# The framework allows this by implementing concurrency inside a single step:
# "When N agents run record-session concurrently" spawns N processes and joins.

Feature: Concurrent agents

  Scenario: Two agents record sessions in the same repo at the same time
    Given a morph repo
    When 2 agents run record-session concurrently
    Then all agents succeeded
    And the repo has exactly 2 run records
