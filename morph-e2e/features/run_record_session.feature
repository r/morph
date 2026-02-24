# Single user: simulate Cursor agent recording a session (MCP morph_record_session).
# One agent runs record-session; we assert objects and runs dirs exist.

Feature: Run record-session (single agent)

  Scenario: One agent records a session
    Given a morph repo
    When I run record-session with prompt "user request" and response "agent reply"
    And the last command succeeded
    Then the path ".morph/objects" exists as a directory
    And the path ".morph/runs" exists as a directory
    And the path ".morph/traces" exists as a directory
