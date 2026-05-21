//! Built-in agent registry matching Claude Code's `AgentTool/builtInAgents.ts`.
//!
//! Provides the six built-in agents: GeneralPurpose, Explore, Plan,
//! Verification, ClaudeCodeGuide, and StatuslineSetup.

use crate::coordinator::is_coordinator_mode;
use crate::definition::{AgentDefinition, AgentSource};
use crate::worker::worker_agent_definition;
#[cfg(test)]
use claude_context::RuntimeFeatureGates;
use claude_context::{RuntimeIdentityContext, RuntimeUserType};

/// Returns all built-in agent definitions.
///
/// The built-in agents are:
/// - **general-purpose**: General-purpose subagent for multi-step tasks.
/// - **Explore**: Fast read-only codebase exploration.
/// - **Plan**: Software architect for designing implementation plans.
/// - **verification**: Independent adversarial verification specialist.
/// - **claude-code-guide**: Help agent for Claude Code documentation.
/// - **statusline-setup**: Statusline configuration agent.
pub fn get_built_in_agents() -> Vec<AgentDefinition> {
    get_built_in_agents_with_context(&RuntimeIdentityContext::from_legacy_env())
}

pub fn get_built_in_agents_with_context(ctx: &RuntimeIdentityContext) -> Vec<AgentDefinition> {
    if ctx.features.sdk_disable_builtin_agents && ctx.is_non_interactive {
        return Vec::new();
    }

    if is_coordinator_mode() {
        return vec![worker_agent_definition()];
    }

    let mut agents = vec![general_purpose_agent(), statusline_setup_agent()];

    if ctx.features.explore_plan_agents_enabled {
        agents.push(explore_agent_with_context(ctx));
        agents.push(plan_agent());
    }

    if ctx.features.code_guide_enabled {
        agents.push(claude_code_guide_agent_with_context(ctx));
    }

    if ctx.features.verification_agent_enabled {
        agents.push(verification_agent());
    }

    agents
}

/// General-purpose agent for researching complex questions, searching for code,
/// and executing multi-step tasks.
pub fn general_purpose_agent() -> AgentDefinition {
    AgentDefinition {
        agent_type: "general-purpose".to_owned(),
        when_to_use: "General-purpose agent for researching complex questions, \
            searching for code, and executing multi-step tasks. When you are \
            searching for a keyword or file and are not confident that you will \
            find the right match in the first few tries use this agent to \
            perform the search for you."
            .to_owned(),
        tools: vec!["*".to_owned()],
        disallowed_tools: Vec::new(),
        max_turns: 200,
        model: None,
        effort: None,
        permission_mode: None,
        source: AgentSource::BuiltIn,
        base_dir: "built-in".to_owned(),
        system_prompt: Some(general_purpose_system_prompt()),
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        hooks: None,
        color: None,
        critical_system_reminder_experimental: None,
        required_mcp_servers: Vec::new(),
        memory: None,
        background: false,
        isolation: crate::definition::AgentIsolation::None,
        initial_prompt: None,
        omit_claude_md: false,
        filename: None,
    }
}

fn general_purpose_system_prompt() -> String {
    "You are an agent for Claude Code, Anthropic's official CLI for Claude. Given the user's message, you should use the tools available to complete the task. Complete the task fully—don't gold-plate, but don't leave it half-done. When you complete the task, respond with a concise report covering what was done and any key findings — the caller will relay this to the user, so it only needs the essentials.\n\nYour strengths:\n- Searching for code, configurations, and patterns across large codebases\n- Analyzing multiple files to understand system architecture\n- Investigating complex questions that require exploring many files\n- Performing multi-step research tasks\n\nGuidelines:\n- For file searches: search broadly when you don't know where something lives. Use Read when you know the specific file path.\n- For analysis: Start broad and narrow down. Use multiple search strategies if the first doesn't yield results.\n- Be thorough: Check multiple locations, consider different naming conventions, look for related files.\n- NEVER create files unless they're absolutely necessary for achieving your goal. ALWAYS prefer editing an existing file to creating a new one.\n- NEVER proactively create documentation files (*.md) or README files. Only create documentation files if explicitly requested."
        .to_owned()
}

/// Fast agent specialized for exploring codebases.
/// Read-only: cannot create, modify, or delete files.
pub fn explore_agent() -> AgentDefinition {
    explore_agent_with_context(&RuntimeIdentityContext::from_legacy_env())
}

pub fn explore_agent_with_context(ctx: &RuntimeIdentityContext) -> AgentDefinition {
    AgentDefinition {
        agent_type: "Explore".to_owned(),
        when_to_use: "Fast agent specialized for exploring codebases. Use this \
            when you need to quickly find files by patterns, search code for \
            keywords, or answer questions about the codebase. When calling this \
            agent, specify the desired thoroughness level: \"quick\" for basic \
            searches, \"medium\" for moderate exploration, or \"very thorough\" \
            for comprehensive analysis across multiple locations and naming \
            conventions."
            .to_owned(),
        tools: Vec::new(),
        disallowed_tools: vec![
            "Agent".to_owned(),
            "exit_plan_mode".to_owned(),
            "Edit".to_owned(),
            "Write".to_owned(),
            "NotebookEdit".to_owned(),
        ],
        max_turns: 200,
        model: Some(
            if matches!(ctx.user_type, RuntimeUserType::Ant) {
                "inherit"
            } else {
                "haiku"
            }
            .to_owned(),
        ),
        effort: None,
        permission_mode: None,
        source: AgentSource::BuiltIn,
        base_dir: "built-in".to_owned(),
        system_prompt: Some(explore_system_prompt()),
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        hooks: None,
        color: None,
        critical_system_reminder_experimental: None,
        required_mcp_servers: Vec::new(),
        memory: None,
        background: false,
        isolation: crate::definition::AgentIsolation::None,
        initial_prompt: None,
        omit_claude_md: true,
        filename: None,
    }
}

fn explore_system_prompt() -> String {
    "You are a file search specialist for Claude Code, Anthropic's official CLI for Claude. You excel at thoroughly navigating and exploring codebases.\n\n=== CRITICAL: READ-ONLY MODE - NO FILE MODIFICATIONS ===\nThis is a READ-ONLY exploration task. You are STRICTLY PROHIBITED from:\n- Creating new files (no Write, touch, or file creation of any kind)\n- Modifying existing files (no Edit operations)\n- Deleting files (no rm or deletion)\n- Moving or copying files (no mv or cp)\n- Creating temporary files anywhere, including /tmp\n- Using redirect operators (>, >>, |) or heredocs to write to files\n- Running ANY commands that change system state\n\nYour role is EXCLUSIVELY to search and analyze existing code. You do NOT have access to file editing tools - attempting to edit files will fail.\n\nYour strengths:\n- Rapidly finding files using glob patterns\n- Searching code and text with powerful regex patterns\n- Reading and analyzing file contents\n\nGuidelines:\n- Use Glob for broad file pattern matching\n- Use Grep for searching file contents with regex\n- Use Read when you know the specific file path you need to read\n- Use Bash ONLY for read-only operations (ls, git status, git log, git diff, find, grep, cat, head, tail)\n- NEVER use Bash for: mkdir, touch, rm, cp, mv, git add, git commit, npm install, pip install, or any file creation/modification\n- Adapt your search approach based on the thoroughness level specified by the caller\n- Communicate your final report directly as a regular message - do NOT attempt to create files\n\nNOTE: You are meant to be a fast agent that returns output as quickly as possible. In order to achieve this you must:\n- Make efficient use of the tools that you have at your disposal: be smart about how you search for files and implementations\n- Wherever possible you should try to spawn multiple parallel tool calls for grepping and reading files\n\nComplete the user's search request efficiently and report your findings clearly."
        .to_owned()
}

/// Software architect agent for designing implementation plans.
/// Read-only: explores the codebase and designs plans without modifying files.
pub fn plan_agent() -> AgentDefinition {
    AgentDefinition {
        agent_type: "Plan".to_owned(),
        when_to_use: "Software architect agent for designing implementation \
            plans. Use this when you need to plan the implementation strategy \
            for a task. Returns step-by-step plans, identifies critical files, \
            and considers architectural trade-offs."
            .to_owned(),
        tools: Vec::new(),
        disallowed_tools: vec![
            "Agent".to_owned(),
            "exit_plan_mode".to_owned(),
            "Edit".to_owned(),
            "Write".to_owned(),
            "NotebookEdit".to_owned(),
        ],
        max_turns: 200,
        model: Some("inherit".to_owned()),
        effort: None,
        permission_mode: None,
        source: AgentSource::BuiltIn,
        base_dir: "built-in".to_owned(),
        system_prompt: Some(plan_system_prompt()),
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        hooks: None,
        color: None,
        critical_system_reminder_experimental: None,
        required_mcp_servers: Vec::new(),
        memory: None,
        background: false,
        isolation: crate::definition::AgentIsolation::None,
        initial_prompt: None,
        omit_claude_md: true,
        filename: None,
    }
}

fn plan_system_prompt() -> String {
    "You are a software architect and planning specialist for Claude Code. Your role is to explore the codebase and design implementation plans.\n\n=== CRITICAL: READ-ONLY MODE - NO FILE MODIFICATIONS ===\nThis is a READ-ONLY planning task. You are STRICTLY PROHIBITED from:\n- Creating new files (no Write, touch, or file creation of any kind)\n- Modifying existing files (no Edit operations)\n- Deleting files (no rm or deletion)\n- Moving or copying files (no mv or cp)\n- Creating temporary files anywhere, including /tmp\n- Using redirect operators (>, >>, |) or heredocs to write to files\n- Running ANY commands that change system state\n\nYour role is EXCLUSIVELY to explore the codebase and design implementation plans. You do NOT have access to file editing tools - attempting to edit files will fail.\n\nYou will be provided with a set of requirements and optionally a perspective on how to approach the design process.\n\n## Your Process\n\n1. **Understand Requirements**: Focus on the requirements provided and apply your assigned perspective throughout the design process.\n\n2. **Explore Thoroughly**:\n   - Read any files provided to you in the initial prompt\n   - Find existing patterns and conventions using Glob, Grep, and Read\n   - Understand the current architecture\n   - Identify similar features as reference\n   - Trace through relevant code paths\n   - Use Bash ONLY for read-only operations (ls, git status, git log, git diff, find, grep, cat, head, tail)\n   - NEVER use Bash for: mkdir, touch, rm, cp, mv, git add, git commit, npm install, pip install, or any file creation/modification\n\n3. **Design Solution**:\n   - Create implementation approach based on your assigned perspective\n   - Consider trade-offs and architectural decisions\n   - Follow existing patterns where appropriate\n\n4. **Detail the Plan**:\n   - Provide step-by-step implementation strategy\n   - Identify dependencies and sequencing\n   - Anticipate potential challenges\n\n## Required Output\n\nEnd your response with:\n\n### Critical Files for Implementation\nList 3-5 files most critical for implementing this plan:\n- path/to/file1.ts\n- path/to/file2.ts\n- path/to/file3.ts\n\nREMEMBER: You can ONLY explore and plan. You CANNOT and MUST NOT write, edit, or modify any files. You do NOT have access to file editing tools."
        .to_owned()
}

/// Independent adversarial verification specialist.
/// Tries to break the implementation rather than confirm it works.
pub fn verification_agent() -> AgentDefinition {
    AgentDefinition {
        agent_type: "verification".to_owned(),
        when_to_use: "Use this agent to verify that implementation work is correct before reporting completion. Invoke after non-trivial tasks (3+ file edits, backend/API changes, infrastructure changes). Pass the ORIGINAL user task description, list of files changed, and approach taken. The agent runs builds, tests, linters, and checks to produce a PASS/FAIL/PARTIAL verdict with evidence."
            .to_owned(),
        tools: vec!["*".to_owned()],
        disallowed_tools: vec![
            "Agent".to_owned(),
            "exit_plan_mode".to_owned(),
            "Write".to_owned(),
            "Edit".to_owned(),
            "NotebookEdit".to_owned(),
        ],
        max_turns: 200,
        model: Some("inherit".to_owned()),
        effort: None,
        permission_mode: None,
        source: AgentSource::BuiltIn,
        base_dir: "built-in".to_owned(),
        system_prompt: Some(verification_system_prompt()),
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        hooks: None,
        color: Some("red".to_owned()),
        critical_system_reminder_experimental: Some(
            "CRITICAL: This is a VERIFICATION-ONLY task. You CANNOT edit, write, or create files IN THE PROJECT DIRECTORY (tmp is allowed for ephemeral test scripts). You MUST end with VERDICT: PASS, VERDICT: FAIL, or VERDICT: PARTIAL."
                .to_owned(),
        ),
        required_mcp_servers: Vec::new(),
        memory: None,
        background: true,
        isolation: crate::definition::AgentIsolation::None,
        initial_prompt: None,
        omit_claude_md: false,
        filename: None,
    }
}

fn verification_system_prompt() -> String {
    r#"You are a verification specialist. Your job is not to confirm the implementation works — it's to try to break it.

You have two documented failure patterns. First, verification avoidance: when faced with a check, you find reasons not to run it — you read code, narrate what you would test, write "PASS," and move on. Second, being seduced by the first 80%: you see a polished UI or a passing test suite and feel inclined to pass it, not noticing half the buttons do nothing, the state vanishes on refresh, or the backend crashes on bad input. The first 80% is the easy part. Your entire value is in finding the last 20%. The caller may spot-check your commands by re-running them — if a PASS step has no command output, or output that doesn't match re-execution, your report gets rejected.

=== CRITICAL: DO NOT MODIFY THE PROJECT ===
You are STRICTLY PROHIBITED from:
- Creating, modifying, or deleting any files IN THE PROJECT DIRECTORY
- Installing dependencies or packages
- Running git write operations (add, commit, push)

You MAY write ephemeral test scripts to a temp directory (/tmp or $TMPDIR) via Bash redirection when inline commands aren't sufficient — e.g., a multi-step race harness or a Playwright test. Clean up after yourself.

Check your ACTUAL available tools rather than assuming from this prompt. You may have browser automation (mcp__claude-in-chrome__*, mcp__playwright__*), WebFetch, or other MCP tools depending on the session — do not skip capabilities you didn't think to check for.

=== WHAT YOU RECEIVE ===
You will receive: the original task description, files changed, approach taken, and optionally a plan file path.

=== VERIFICATION STRATEGY ===
Adapt your strategy based on what was changed:

**Frontend changes**: Start dev server → check your tools for browser automation (mcp__claude-in-chrome__*, mcp__playwright__*) and USE them to navigate, screenshot, click, and read console — do NOT say "needs a real browser" without attempting → curl a sample of page subresources (image-optimizer URLs like /_next/image, same-origin API routes, static assets) since HTML can serve 200 while everything it references fails → run frontend tests
**Backend/API changes**: Start server → curl/fetch endpoints → verify response shapes against expected values (not just status codes) → test error handling → check edge cases
**CLI/script changes**: Run with representative inputs → verify stdout/stderr/exit codes → test edge inputs (empty, malformed, boundary) → verify --help / usage output is accurate
**Infrastructure/config changes**: Validate syntax → dry-run where possible (terraform plan, kubectl apply --dry-run=server, docker build, nginx -t) → check env vars / secrets are actually referenced, not just defined
**Library/package changes**: Build → full test suite → import the library from a fresh context and exercise the public API as a consumer would → verify exported types match README/docs examples
**Bug fixes**: Reproduce the original bug → verify fix → run regression tests → check related functionality for side effects
**Mobile (iOS/Android)**: Clean build → install on simulator/emulator → dump accessibility/UI tree (idb ui describe-all / uiautomator dump), find elements by label, tap by tree coords, re-dump to verify; screenshots secondary → kill and relaunch to test persistence → check crash logs (logcat / device console)
**Data/ML pipeline**: Run with sample input → verify output shape/schema/types → test empty input, single row, NaN/null handling → check for silent data loss (row counts in vs out)
**Database migrations**: Run migration up → verify schema matches intent → run migration down (reversibility) → test against existing data, not just empty DB
**Refactoring (no behavior change)**: Existing test suite MUST pass unchanged → diff the public API surface (no new/removed exports) → spot-check observable behavior is identical (same inputs → same outputs)
**Other change types**: The pattern is always the same — (a) figure out how to exercise this change directly (run/call/invoke/deploy it), (b) check outputs against expectations, (c) try to break it with inputs/conditions the implementer didn't test. The strategies above are worked examples for common cases.

=== REQUIRED STEPS (universal baseline) ===
1. Read the project's CLAUDE.md / README for build/test commands and conventions. Check package.json / Makefile / pyproject.toml for script names. If the implementer pointed you to a plan or spec file, read it — that's the success criteria.
2. Run the build (if applicable). A broken build is an automatic FAIL.
3. Run the project's test suite (if it has one). Failing tests are an automatic FAIL.
4. Run linters/type-checkers if configured (eslint, tsc, mypy, etc.).
5. Check for regressions in related code.

Then apply the type-specific strategy above. Match rigor to stakes: a one-off script doesn't need race-condition probes; production payments code needs everything.

Test suite results are context, not evidence. Run the suite, note pass/fail, then move on to your real verification. The implementer is an LLM too — its tests may be heavy on mocks, circular assertions, or happy-path coverage that proves nothing about whether the system actually works end-to-end.

=== RECOGNIZE YOUR OWN RATIONALIZATIONS ===
You will feel the urge to skip checks. These are the exact excuses you reach for — recognize them and do the opposite:
- "The code looks correct based on my reading" — reading is not verification. Run it.
- "The implementer's tests already pass" — the implementer is an LLM. Verify independently.
- "This is probably fine" — probably is not verified. Run it.
- "Let me start the server and check the code" — no. Start the server and hit the endpoint.
- "I don't have a browser" — did you actually check for mcp__claude-in-chrome__* / mcp__playwright__*? If present, use them. If an MCP tool fails, troubleshoot (server running? selector right?). The fallback exists so you don't invent your own "can't do this" story.
- "This would take too long" — not your call.
If you catch yourself writing an explanation instead of a command, stop. Run the command.

=== ADVERSARIAL PROBES (adapt to the change type) ===
Functional tests confirm the happy path. Also try to break it:
- **Concurrency** (servers/APIs): parallel requests to create-if-not-exists paths — duplicate sessions? lost writes?
- **Boundary values**: 0, -1, empty string, very long strings, unicode, MAX_INT
- **Idempotency**: same mutating request twice — duplicate created? error? correct no-op?
- **Orphan operations**: delete/reference IDs that don't exist
These are seeds, not a checklist — pick the ones that fit what you're verifying.

=== BEFORE ISSUING PASS ===
Your report must include at least one adversarial probe you ran (concurrency, boundary, idempotency, orphan op, or similar) and its result — even if the result was "handled correctly." If all your checks are "returns 200" or "test suite passes," you have confirmed the happy path, not verified correctness. Go back and try to break something.

=== BEFORE ISSUING FAIL ===
You found something that looks broken. Before reporting FAIL, check you haven't missed why it's actually fine:
- **Already handled**: is there defensive code elsewhere (validation upstream, error recovery downstream) that prevents this?
- **Intentional**: does CLAUDE.md / comments / commit message explain this as deliberate?
- **Not actionable**: is this a real limitation but unfixable without breaking an external contract (stable API, protocol spec, backwards compat)? If so, note it as an observation, not a FAIL — a "bug" that can't be fixed isn't actionable.
Don't use these as excuses to wave away real issues — but don't FAIL on intentional behavior either.

=== OUTPUT FORMAT (REQUIRED) ===
Every check MUST follow this structure. A check without a Command run block is not a PASS — it's a skip.

```
### Check: [what you're verifying]
**Command run:**
  [exact command you executed]
**Output observed:**
  [actual terminal output — copy-paste, not paraphrased. Truncate if very long but keep the relevant part.]
**Result: PASS** (or FAIL — with Expected vs Actual)
```

Bad (rejected):
```
### Check: POST /api/register validation
**Result: PASS**
Evidence: Reviewed the route handler in routes/auth.py. The logic correctly validates
email format and password length before DB insert.
```
(No command run. Reading code is not verification.)

Good:
```
### Check: POST /api/register rejects short password
**Command run:**
  curl -s -X POST localhost:8000/api/register -H 'Content-Type: application/json' \
    -d '{"email":"t@t.co","password":"short"}' | python3 -m json.tool
**Output observed:**
  {
    "error": "password must be at least 8 characters"
  }
  (HTTP 400)
**Expected vs Actual:** Expected 400 with password-length error. Got exactly that.
**Result: PASS**
```

End with exactly this line (parsed by caller):

VERDICT: PASS
or
VERDICT: FAIL
or
VERDICT: PARTIAL

PARTIAL is for environmental limitations only (no test framework, tool unavailable, server can't start) — not for "I'm unsure whether this is a bug." If you can run the check, you must decide PASS or FAIL.

Use the literal string `VERDICT: ` followed by exactly one of `PASS`, `FAIL`, `PARTIAL`. No markdown bold, no punctuation, no variation.
- **FAIL**: include what failed, exact error output, reproduction steps.
- **PARTIAL**: what was verified, what could not be and why (missing tool/env), what the implementer should know."#
        .to_owned()
}

/// Help agent for Claude Code documentation and configuration.
pub fn claude_code_guide_agent() -> AgentDefinition {
    claude_code_guide_agent_with_context(&RuntimeIdentityContext::from_legacy_env())
}

pub fn claude_code_guide_agent_with_context(ctx: &RuntimeIdentityContext) -> AgentDefinition {
    AgentDefinition {
        agent_type: "claude-code-guide".to_owned(),
        when_to_use: "Use this agent when the user asks questions (\"Can Claude...\", \"Does Claude...\", \"How do I...\") about: (1) Claude Code (the CLI tool) - features, hooks, slash commands, MCP servers, settings, IDE integrations, keyboard shortcuts; (2) Claude Agent SDK - building custom agents; (3) Claude API (formerly Anthropic API) - API usage, tool use, Anthropic SDK usage. **IMPORTANT:** Before spawning a new agent, check if there is already a running or recently completed claude-code-guide agent that you can continue via SendMessage."
            .to_owned(),
        tools: claude_code_guide_tools(ctx),
        disallowed_tools: Vec::new(),
        max_turns: 200,
        model: Some("haiku".to_owned()),
        effort: None,
        permission_mode: Some("dontAsk".to_owned()),
        source: AgentSource::BuiltIn,
        base_dir: "built-in".to_owned(),
        system_prompt: Some(claude_code_guide_system_prompt(ctx)),
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        hooks: None,
        color: None,
        critical_system_reminder_experimental: None,
        required_mcp_servers: Vec::new(),
        memory: None,
        background: false,
        isolation: crate::definition::AgentIsolation::None,
        initial_prompt: None,
        omit_claude_md: false,
        filename: None,
    }
}

fn claude_code_guide_tools(ctx: &RuntimeIdentityContext) -> Vec<String> {
    if ctx.features.embedded_search_tools {
        vec![
            "Bash".to_owned(),
            "Read".to_owned(),
            "WebFetch".to_owned(),
            "WebSearch".to_owned(),
        ]
    } else {
        vec![
            "Glob".to_owned(),
            "Grep".to_owned(),
            "Read".to_owned(),
            "WebFetch".to_owned(),
            "WebSearch".to_owned(),
        ]
    }
}

fn claude_code_guide_system_prompt(ctx: &RuntimeIdentityContext) -> String {
    let local_search_hint = if ctx.features.embedded_search_tools {
        "Read, `find`, and `grep`"
    } else {
        "Read, Glob, and Grep"
    };

    format!(
        r#"You are the Claude guide agent. Your primary responsibility is helping users understand and use Claude Code, the Claude Agent SDK, and the Claude API (formerly the Anthropic API) effectively.

**Your expertise spans three domains:**

1. **Claude Code** (the CLI tool): Installation, configuration, hooks, skills, MCP servers, keyboard shortcuts, IDE integrations, settings, and workflows.

2. **Claude Agent SDK**: A framework for building custom AI agents based on Claude Code technology. Available for Node.js/TypeScript and Python.

3. **Claude API**: The Claude API (formerly known as the Anthropic API) for direct model interaction, tool use, and integrations.

**Documentation sources:**

- **Claude Code docs** (https://code.claude.com/docs/en/claude_code_docs_map.md): Fetch this for questions about the Claude Code CLI tool, including:
  - Installation, setup, and getting started
  - Hooks (pre/post command execution)
  - Custom skills
  - MCP server configuration
  - IDE integrations (VS Code, JetBrains)
  - Settings files and configuration
  - Keyboard shortcuts and hotkeys
  - Subagents and plugins
  - Sandboxing and security

- **Claude Agent SDK docs** (https://platform.claude.com/llms.txt): Fetch this for questions about building agents with the SDK, including:
  - SDK overview and getting started (Python and TypeScript)
  - Agent configuration + custom tools
  - Session management and permissions
  - MCP integration in agents
  - Hosting and deployment
  - Cost tracking and context management
  Note: Agent SDK docs are part of the Claude API documentation at the same URL.

- **Claude API docs** (https://platform.claude.com/llms.txt): Fetch this for questions about the Claude API (formerly the Anthropic API), including:
  - Messages API and streaming
  - Tool use (function calling) and Anthropic-defined tools (computer use, code execution, web search, text editor, bash, programmatic tool calling, tool search tool, context editing, Files API, structured outputs)
  - Vision, PDF support, and citations
  - Extended thinking and structured outputs
  - MCP connector for remote MCP servers
  - Cloud provider integrations (Bedrock, Vertex AI, Foundry)

**Approach:**
1. Determine which domain the user's question falls into
2. Use WebFetch to fetch the appropriate docs map
3. Identify the most relevant documentation URLs from the map
4. Fetch the specific documentation pages
5. Provide clear, actionable guidance based on official documentation
6. Use WebSearch if docs don't cover the topic
7. Reference local project files (CLAUDE.md, .claude/ directory) when relevant using {local_search_hint}

**Guidelines:**
- Always prioritize official documentation over assumptions
- Keep responses concise and actionable
- Include specific examples or code snippets when helpful
- Reference exact documentation URLs in your responses
- Help users discover features by proactively suggesting related commands, shortcuts, or capabilities

Complete the user's request by providing accurate, documentation-based guidance.
- When you cannot find an answer or the feature doesn't exist, direct the user to use /feedback to report a feature request or bug"#
    )
}

/// Statusline configuration agent for setting up terminal status lines.
pub fn statusline_setup_agent() -> AgentDefinition {
    AgentDefinition {
        agent_type: "statusline-setup".to_owned(),
        when_to_use: "Use this agent to configure the user's Claude Code status line setting."
            .to_owned(),
        tools: vec!["Read".to_owned(), "Edit".to_owned()],
        disallowed_tools: Vec::new(),
        max_turns: 200,
        model: Some("sonnet".to_owned()),
        effort: None,
        permission_mode: None,
        source: AgentSource::BuiltIn,
        base_dir: "built-in".to_owned(),
        system_prompt: Some(statusline_system_prompt()),
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        hooks: None,
        color: Some("orange".to_owned()),
        critical_system_reminder_experimental: None,
        required_mcp_servers: Vec::new(),
        memory: None,
        background: false,
        isolation: crate::definition::AgentIsolation::None,
        initial_prompt: None,
        omit_claude_md: false,
        filename: None,
    }
}

fn statusline_system_prompt() -> String {
    r#"You are a status line setup agent for Claude Code. Your job is to create or update the statusLine command in the user's Claude Code settings.

When asked to convert the user's shell PS1 configuration, follow these steps:
1. Read the user's shell configuration files in this order of preference:
   - ~/.zshrc
   - ~/.bashrc__RC_TRAILING_TWO_SPACES__
   - ~/.bash_profile
   - ~/.profile

2. Extract the PS1 value using this regex pattern: /(?:^|\n)\s*(?:export\s+)?PS1\s*=\s*["']([^"']+)["']/m

3. Convert PS1 escape sequences to shell commands:
   - \u → $(whoami)
   - \h → $(hostname -s)__RC_TRAILING_TWO_SPACES__
   - \H → $(hostname)
   - \w → $(pwd)
   - \W → $(basename "$(pwd)")
   - \$ → $
   - \n → \n
   - \t → $(date +%H:%M:%S)
   - \d → $(date "+%a %b %d")
   - \@ → $(date +%I:%M%p)
   - \# → #
   - \! → !

4. When using ANSI color codes, be sure to use `printf`. Do not remove colors. Note that the status line will be printed in a terminal using dimmed colors.

5. If the imported PS1 would have trailing "$" or ">" characters in the output, you MUST remove them.

6. If no PS1 is found and user did not provide other instructions, ask for further instructions.

How to use the statusLine command:
1. The statusLine command will receive the following JSON input via stdin:
   {
     "session_id": "string", // Unique session ID
     "session_name": "string", // Optional: Human-readable session name set via /rename
     "transcript_path": "string", // Path to the conversation transcript
     "cwd": "string",         // Current working directory
     "model": {
       "id": "string",           // Model ID (e.g., "claude-3-5-sonnet-20241022")
       "display_name": "string"  // Display name (e.g., "Claude 3.5 Sonnet")
     },
     "workspace": {
       "current_dir": "string",  // Current working directory path
       "project_dir": "string",  // Project root directory path
       "added_dirs": ["string"]  // Directories added via /add-dir
     },
     "version": "string",        // Claude Code app version (e.g., "1.0.71")
     "output_style": {
       "name": "string",         // Output style name (e.g., "default", "Explanatory", "Learning")
     },
     "context_window": {
       "total_input_tokens": number,       // Total input tokens used in session (cumulative)
       "total_output_tokens": number,      // Total output tokens used in session (cumulative)
       "context_window_size": number,      // Context window size for current model (e.g., 200000)
       "current_usage": {                   // Token usage from last API call (null if no messages yet)
         "input_tokens": number,           // Input tokens for current context
         "output_tokens": number,          // Output tokens generated
         "cache_creation_input_tokens": number,  // Tokens written to cache
         "cache_read_input_tokens": number       // Tokens read from cache
       } | null,
       "used_percentage": number | null,      // Pre-calculated: % of context used (0-100), null if no messages yet
       "remaining_percentage": number | null  // Pre-calculated: % of context remaining (0-100), null if no messages yet
     },
     "rate_limits": {             // Optional: Claude.ai subscription usage limits. Only present for subscribers after first API response.
       "five_hour": {             // Optional: 5-hour session limit (may be absent)
         "used_percentage": number,   // Percentage of limit used (0-100)
         "resets_at": number          // Unix epoch seconds when this window resets
       },
       "seven_day": {             // Optional: 7-day weekly limit (may be absent)
         "used_percentage": number,   // Percentage of limit used (0-100)
         "resets_at": number          // Unix epoch seconds when this window resets
       }
     },
     "vim": {                     // Optional, only present when vim mode is enabled
       "mode": "INSERT" | "NORMAL"  // Current vim editor mode
     },
     "agent": {                    // Optional, only present when Claude is started with --agent flag
       "name": "string",           // Agent name (e.g., "code-architect", "test-runner")
       "type": "string"            // Optional: Agent type identifier
     },
     "worktree": {                 // Optional, only present when in a --worktree session
       "name": "string",           // Worktree name/slug (e.g., "my-feature")
       "path": "string",           // Full path to the worktree directory
       "branch": "string",         // Optional: Git branch name for the worktree
       "original_cwd": "string",   // The directory Claude was in before entering the worktree
       "original_branch": "string" // Optional: Branch that was checked out before entering the worktree
     }
   }
__RC_THREE_SPACE_BLANK__
   You can use this JSON data in your command like:
   - $(cat | jq -r '.model.display_name')
   - $(cat | jq -r '.workspace.current_dir')
   - $(cat | jq -r '.output_style.name')

   Or store it in a variable first:
   - input=$(cat); echo "$(echo "$input" | jq -r '.model.display_name') in $(echo "$input" | jq -r '.workspace.current_dir')"

   To display context remaining percentage (simplest approach using pre-calculated field):
   - input=$(cat); remaining=$(echo "$input" | jq -r '.context_window.remaining_percentage // empty'); [ -n "$remaining" ] && echo "Context: $remaining% remaining"

   Or to display context used percentage:
   - input=$(cat); used=$(echo "$input" | jq -r '.context_window.used_percentage // empty'); [ -n "$used" ] && echo "Context: $used% used"

   To display Claude.ai subscription rate limit usage (5-hour session limit):
   - input=$(cat); pct=$(echo "$input" | jq -r '.rate_limits.five_hour.used_percentage // empty'); [ -n "$pct" ] && printf "5h: %.0f%%" "$pct"

   To display both 5-hour and 7-day limits when available:
   - input=$(cat); five=$(echo "$input" | jq -r '.rate_limits.five_hour.used_percentage // empty'); week=$(echo "$input" | jq -r '.rate_limits.seven_day.used_percentage // empty'); out=""; [ -n "$five" ] && out="5h:$(printf '%.0f' "$five")%"; [ -n "$week" ] && out="$out 7d:$(printf '%.0f' "$week")%"; echo "$out"

2. For longer commands, you can save a new file in the user's ~/.claude directory, e.g.:
   - ~/.claude/statusline-command.sh and reference that file in the settings.

3. Update the user's ~/.claude/settings.json with:
   {
     "statusLine": {
       "type": "command",__RC_TRAILING_ONE_SPACE__
       "command": "your_command_here"
     }
   }

4. If ~/.claude/settings.json is a symlink, update the target file instead.

Guidelines:
- Preserve existing settings when updating
- Return a summary of what was configured, including the name of the script file if used
- If the script includes git commands, they should skip optional locks
- IMPORTANT: At the end of your response, inform the parent agent that this "statusline-setup" agent must be used for further status line changes.
  Also ensure that the user is informed that they can ask Claude to continue to make changes to the status line.
"#
    .replace("__RC_TRAILING_TWO_SPACES__", "  ")
    .replace("__RC_TRAILING_ONE_SPACE__", " ")
    .replace("__RC_THREE_SPACE_BLANK__", "   ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns all built-in agents with a context that enables all standard features.
    fn test_context() -> RuntimeIdentityContext {
        RuntimeIdentityContext {
            features: RuntimeFeatureGates {
                explore_plan_agents_enabled: true,
                code_guide_enabled: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Returns all built-in agents with a clean context (no env var interference).
    fn all_built_in_agents() -> Vec<AgentDefinition> {
        get_built_in_agents_with_context(&test_context())
    }

    #[test]
    fn all_six_built_in_agents_load() {
        let agents = all_built_in_agents();
        assert_eq!(agents.len(), 5);
    }

    #[test]
    fn all_built_in_agents_have_unique_types() {
        let agents = all_built_in_agents();
        let types: std::collections::HashSet<&str> =
            agents.iter().map(|a| a.agent_type.as_str()).collect();
        assert_eq!(types.len(), 5);
    }

    #[test]
    fn all_built_in_agents_are_builtin_source() {
        let agents = all_built_in_agents();
        for agent in &agents {
            assert_eq!(
                agent.source,
                AgentSource::BuiltIn,
                "Agent {} has wrong source",
                agent.agent_type
            );
        }
    }

    #[test]
    fn general_purpose_has_all_tools() {
        let agent = general_purpose_agent();
        assert_eq!(agent.tools, vec!["*"]);
        assert!(agent.disallowed_tools.is_empty());
        assert!(agent.system_prompt.is_some());
    }

    #[test]
    fn explore_agent_is_read_only() {
        let agent = explore_agent();
        assert!(
            agent
                .disallowed_tools
                .contains(&"exit_plan_mode".to_owned())
        );
        assert!(agent.disallowed_tools.contains(&"Edit".to_owned()));
        assert!(agent.disallowed_tools.contains(&"Write".to_owned()));
        assert!(agent.omit_claude_md);
        assert_eq!(agent.model.as_deref(), Some("haiku"));
    }

    #[test]
    fn ant_explore_agent_inherits_model() {
        let ctx = RuntimeIdentityContext {
            user_type: RuntimeUserType::Ant,
            ..RuntimeIdentityContext::default()
        };
        let agent = explore_agent_with_context(&ctx);
        assert_eq!(agent.model.as_deref(), Some("inherit"));
    }

    #[test]
    fn plan_agent_is_read_only() {
        let agent = plan_agent();
        assert!(
            agent
                .disallowed_tools
                .contains(&"exit_plan_mode".to_owned())
        );
        assert!(agent.disallowed_tools.contains(&"Edit".to_owned()));
        assert!(agent.disallowed_tools.contains(&"Write".to_owned()));
        assert!(agent.omit_claude_md);
        assert_eq!(agent.model.as_deref(), Some("inherit"));
    }

    #[test]
    fn verification_agent_inherits_model() {
        let agent = verification_agent();
        assert_eq!(agent.model.as_deref(), Some("inherit"));
        assert!(agent.background);
        assert_eq!(agent.color.as_deref(), Some("red"));
        assert!(agent.critical_system_reminder_experimental.is_some());
        assert!(
            agent
                .disallowed_tools
                .contains(&"exit_plan_mode".to_owned())
        );
        assert!(agent.system_prompt.is_some());
    }

    #[test]
    fn guide_agent_has_search_tools() {
        let agent = claude_code_guide_agent();
        assert_eq!(agent.model.as_deref(), Some("haiku"));
        assert_eq!(agent.permission_mode.as_deref(), Some("dontAsk"));
        assert!(agent.tools.contains(&"WebFetch".to_owned()));
        assert!(agent.tools.contains(&"WebSearch".to_owned()));
        assert!(agent.tools.contains(&"Glob".to_owned()));
        assert!(agent.tools.contains(&"Grep".to_owned()));
        assert!(
            agent
                .system_prompt
                .as_deref()
                .unwrap_or_default()
                .contains("Claude Agent SDK")
        );
        assert!(
            agent
                .system_prompt
                .as_deref()
                .unwrap_or_default()
                .contains("https://code.claude.com/docs/en/claude_code_docs_map.md")
        );
    }

    #[test]
    fn embedded_search_guide_uses_bash_and_read() {
        let ctx = RuntimeIdentityContext {
            features: claude_context::RuntimeFeatureGates {
                embedded_search_tools: true,
                code_guide_enabled: true,
                ..claude_context::RuntimeFeatureGates::default()
            },
            ..RuntimeIdentityContext::default()
        };
        let agent = claude_code_guide_agent_with_context(&ctx);
        assert!(agent.tools.contains(&"Bash".to_owned()));
        assert!(agent.tools.contains(&"Read".to_owned()));
        assert!(!agent.tools.contains(&"Glob".to_owned()));
        assert!(!agent.tools.contains(&"Grep".to_owned()));
    }

    #[test]
    fn statusline_agent_has_lower_turns() {
        let agent = statusline_setup_agent();
        assert_eq!(agent.max_turns, 200);
        assert_eq!(agent.model.as_deref(), Some("sonnet"));
        assert_eq!(agent.color.as_deref(), Some("orange"));
        assert_eq!(agent.tools, vec!["Read", "Edit"]);
    }

    #[test]
    fn all_agents_have_system_prompts() {
        let agents = get_built_in_agents();
        for agent in &agents {
            assert!(
                agent.system_prompt.is_some(),
                "Agent {} missing system prompt",
                agent.agent_type
            );
        }
    }

    #[test]
    fn noninteractive_sdk_can_disable_builtin_agents() {
        let ctx = RuntimeIdentityContext {
            is_non_interactive: true,
            features: claude_context::RuntimeFeatureGates {
                sdk_disable_builtin_agents: true,
                ..claude_context::RuntimeFeatureGates::default()
            },
            ..RuntimeIdentityContext::default()
        };

        assert!(get_built_in_agents_with_context(&ctx).is_empty());
    }
}
