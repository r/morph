Feature: Hosted inspection workflow
  Teams use the Morph hosted service to inspect commits,
  runs, traces, pipelines, and behavioral status through
  a stable HTTP/API surface.

  Scenario: Team inspects a certified commit through the hosted service
    Given a morph repo
    And a file "src/main.py" with content "print('hello')"
    When I run "morph add ."
    Then the last command succeeded
    When I run "morph commit -m initial --metrics {\"acc\":0.95,\"f1\":0.88} --json"
    Then the last command succeeded
    When I capture the last output as "commit_hash"
    When I create a JSON file "metrics.json" with metrics "acc=0.95,f1=0.88"
    When I run "morph certify --metrics-file metrics.json --runner ci-bot"
    Then the last command succeeded
    When I start the morph server on port "19871"
    And I query the server at "/api/repos/default/commits/<commit_hash>"
    Then the JSON response field "behavioral_status.certified" equals "true"
    And the JSON response field "behavioral_status.certification.runner" equals "ci-bot"
    And the JSON response field "eval_contract.observed_metrics.acc" equals "0.95"
    When I query the server at "/api/repos/default/summary"
    Then the JSON response field "commit_count" equals "1"
    And I stop the morph server

  Scenario: Team inspects merge status through the hosted service
    Given a morph repo
    And the identity pipeline and a minimal eval suite exist
    And a file "a.txt" with content "aaa"
    When I run "morph add ."
    Then the last command succeeded
    When I run "morph pipeline create prog.json"
    And I capture the last output as "prog_hash"
    When I run "morph commit -m main-commit --pipeline <prog_hash> --metrics {\"acc\":0.8}"
    Then the last command succeeded
    When I run "morph branch feature"
    And I run "morph checkout feature"
    Then the last command succeeded
    When I run "morph commit -m feature-commit --pipeline <prog_hash> --metrics {\"acc\":0.85}"
    Then the last command succeeded
    When I run "morph checkout main"
    Then the last command succeeded
    When I run "morph merge feature -m merged --pipeline <prog_hash> --metrics {\"acc\":0.9}"
    Then the last command succeeded
    When I capture the last output as "merge_hash"
    When I start the morph server on port "19872"
    And I query the server at "/api/repos/default/commits/<merge_hash>"
    Then the JSON response field "behavioral_status.is_merge" equals "true"
    And the JSON response field "behavioral_status.merge_status.dominates_a" is present
    And the JSON response field "behavioral_status.merge_status.dominates_b" is present
    And I stop the morph server

  Scenario: Team inspects extracted pipeline and source run
    Given a morph repo
    And a file "code.py" with content "print('hi')"
    When I run "morph add ."
    Then the last command succeeded
    When I run record-session with prompt "Build feature X" and response "Feature X built"
    Then the last command succeeded
    When I capture the last output as "run_hash"
    When I run "morph pipeline extract --from-run <run_hash>"
    Then the last command succeeded
    When I capture the last output as "pipeline_hash"
    When I run "morph commit -m snap"
    Then the last command succeeded
    When I start the morph server on port "19873"
    And I query the server at "/api/repos/default/runs/<run_hash>"
    Then the JSON response field "agent.id" is present
    And the JSON response field "trace" is present
    When I query the server at "/api/repos/default/pipelines/<pipeline_hash>"
    Then the JSON response field "provenance.method" equals "extracted"
    And the JSON response field "provenance.derived_from_run" equals "<run_hash>"
    And the JSON response field "node_count" equals "2"
    And I stop the morph server

  Scenario: Invalid repo or missing object returns clear service error
    Given a morph repo
    When I start the morph server on port "19874"
    And I query the server at "/api/repos/default/commits/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    Then the JSON response code is "404"
    And the JSON response field "code" equals "not_found"
    When I query the server at "/api/repos/nonexistent/summary"
    Then the JSON response code is "404"
    And the JSON response field "code" equals "repo_not_found"
    When I query the server at "/api/repos/default/commits/not-a-hash"
    Then the JSON response code is "400"
    And the JSON response field "code" equals "bad_hash"
    And I stop the morph server
