# Single user: create a prompt blob from a file, then materialize it back.
# We capture the prompt hash and use it in the materialize command.

Feature: Prompt create and materialize

  Scenario: Create prompt from file and materialize to output
    Given a morph repo
    And a file ".morph/prompts/hello.txt" with content "Hello world"
    When I run "morph prompt create .morph/prompts/hello.txt"
    And I capture the last output as "prompt_hash"
    And the last command succeeded
    When I run "morph prompt materialize <prompt_hash> --output .morph/prompts/out.prompt"
    And the last command succeeded
    Then stdout contains "Materialized"
    And the file ".morph/prompts/out.prompt" has content "Hello world"
