# Hook scripts: test that Claude Code and Cursor hook scripts correctly
# record prompts and create Run+Trace objects in the morph store.
# These scripts were broken by the Program→Pipeline rename (83ab660)
# and had no test coverage. This feature prevents future regressions.

Feature: IDE hook scripts record sessions to morph store

  # --- Claude Code hooks ---

  Scenario: Claude Code prompt hook appends to pending JSONL
    Given a morph repo
    When I pipe into hook "claude-code/hooks/morph-record-prompt.sh" the JSON:
      """
      {"cwd":"<REPO>","session_id":"sess-abc123","prompt":"hello world"}
      """
    Then the hook exited successfully
    And the path ".morph/hooks/pending-sess-abc123.jsonl" is present
    And the file ".morph/hooks/pending-sess-abc123.jsonl" contains "hello world"

  Scenario: Claude Code prompt hook handles empty stdin gracefully
    Given a morph repo
    When I pipe into hook "claude-code/hooks/morph-record-prompt.sh" the JSON:
      """
      """
    Then the hook exited successfully

  Scenario: Claude Code stop hook creates run and trace in store
    Given a morph repo
    When I pipe into hook "claude-code/hooks/morph-record-prompt.sh" the JSON:
      """
      {"cwd":"<REPO>","session_id":"sess-full01","prompt":"build a feature"}
      """
    And I pipe into hook "claude-code/hooks/morph-record-stop.sh" the JSON:
      """
      {"cwd":"<REPO>","session_id":"sess-full01","last_assistant_message":"done"}
      """
    Then the hook exited successfully
    And the path ".morph/runs" exists as a directory
    And the path ".morph/traces" exists as a directory
    And the path ".morph/hooks/pending-sess-full01.jsonl" does not exist

  # --- Cursor hooks ---

  Scenario: Cursor prompt hook appends to pending JSONL
    Given a morph repo
    When I pipe into hook "morph-cli/assets/cursor/hooks/morph-record-prompt.sh" the JSON:
      """
      {"workspace_roots":["<REPO>"],"conversation_id":"conv-xyz789","prompt":"fix bug"}
      """
    Then the hook exited successfully
    And the path ".morph/hooks/pending-conv-xyz789.jsonl" is present
    And the file ".morph/hooks/pending-conv-xyz789.jsonl" contains "fix bug"

  Scenario: Cursor response hook creates run and trace in store
    Given a morph repo
    When I pipe into hook "morph-cli/assets/cursor/hooks/morph-record-prompt.sh" the JSON:
      """
      {"workspace_roots":["<REPO>"],"conversation_id":"conv-resp01","prompt":"refactor"}
      """
    And I pipe into hook "morph-cli/assets/cursor/hooks/morph-record-response.sh" the JSON:
      """
      {"workspace_roots":["<REPO>"],"conversation_id":"conv-resp01","text":"refactored the module"}
      """
    Then the hook exited successfully
    And the path ".morph/runs" exists as a directory
    And the path ".morph/traces" exists as a directory
    And the path ".morph/hooks/pending-conv-resp01.jsonl" does not exist

  Scenario: Cursor stop hook creates run and trace in store
    Given a morph repo
    When I pipe into hook "morph-cli/assets/cursor/hooks/morph-record-prompt.sh" the JSON:
      """
      {"workspace_roots":["<REPO>"],"conversation_id":"conv-stop01","prompt":"add tests"}
      """
    And I pipe into hook "morph-cli/assets/cursor/hooks/morph-record-stop.sh" the JSON:
      """
      {"workspace_roots":["<REPO>"],"conversation_id":"conv-stop01"}
      """
    Then the hook exited successfully
    And the path ".morph/runs" exists as a directory
    And the path ".morph/traces" exists as a directory
    And the path ".morph/hooks/pending-conv-stop01.jsonl" does not exist
