# Knowledge: camp_action.implement_set

## Data (SSOT)
```json
{
  "knowledgeSchema": "v1",
  "entry": {
    "id": "camp_action.implement_set",
    "title": "action.implement_set",
    "kind": "camp_action",
    "scope": "global",
    "space": "cfm"
  },
  "binding": {
    "when": {
      "branch": [],
      "action": [],
      "conceptType": [],
      "conceptId": [],
      "nodeId": []
    }
  },
  "source": {
    "summary": "Campaign constructor action"
  },
  "freshness": {
    "state": "unknown"
  }
}
```

## Content
<!-- CF_BLOCK:CONTENT -->
{
  "campaignActionSchema": "v1",
  "id": "implement_set",
  "label": "action.implement_set",
  "description": "Generate phase input/workflow guidance for implementing concepts from one selected CF set using cfm-ops dev-set-act commands and CF developer skills.",
  "enabled": true,
  "version": 1,
  "requiresSet": true,
  "tokens": [
    "SELECTED_SET_ID",
    "SELECTED_SET_LABEL"
  ],
  "templates": {
    "input": "SELECTED_SET = {SELECTED_SET_ID}\n\nWork on the currently selected CF set {SELECTED_SET_LABEL}. This set is the canonical scope for this phase input.\nUse the `cfm-ops-cli` skill to read set-driven context through CFM CLI (backend SSOT).\n\nSuggested command flow (read-only context first):\n- cfm-ops help dev-set-act\n- cfm-ops dev-set-act-set-summary --repo <repoRoot> --set {SELECTED_SET_ID}\n- cfm-ops dev-set-act-set-full --repo <repoRoot> --set {SELECTED_SET_ID}\n\nFor focused deep dives on individual concepts from the set:\n- cfm-ops dev-set-act-concept-base --repo <repoRoot> --ref <node::concept>\n- cfm-ops dev-set-act-concept-full --repo <repoRoot> --ref <node::concept>\n\nTreat the selected set as the working scope input for planning and delivery in this campaign phase.",
    "workflow": "Load and use skill `cf-universal` as the CF model/semantics base (required).\nLoad and use skill `cf-dev-universal` for implementation delivery loop across selected concepts.\n\nWithin this campaign phase:\n- Build/maintain phase plan and success gates in Campaigns UI (backend SSOT).\n- Delegate implementation work against concepts from SELECTED_SET.\n- Request code review after implementation batches.\n- Apply fixes from review and run final audit/verification.\n- Keep the final success gate as summary of completed work + conclusions for this phase.\n\nPrefer deterministic progress updates: reflect plan/gates after each milestone batch."
  },
  "recommendedSkills": {
    "generalKnowledge": [
      "cf-universal"
    ],
    "actions": [
      "cf-dev-universal",
      "cfm-ops-cli"
    ],
    "generalKnowledgeIds": [],
    "actionIds": []
  }
}

<!-- /CF_BLOCK:CONTENT -->

