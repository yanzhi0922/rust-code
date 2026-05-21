//! Detailed tool prompt descriptions for all built-in tools.
//!
//! Each prompt is a static string constant providing the LLM with rich context
//! about when and how to use the tool. Prompts are modelled after Claude Code's
//! prompt.ts files but adapted for the Rust codebase.

use std::path::PathBuf;

use claude_agents::coordinator::is_coordinator_mode;
use claude_agents::fork::is_fork_subagent_enabled;
use claude_agents::loader::load_all_agents_with_context;
use claude_context::RuntimeIdentityContext;
use claude_mcp::normalization::mcp_info_from_string;
use directories::BaseDirs;

// ── Core tools (P0) ──────────────────────────────────────────────────────────

/// Prompt for `list_directory`.
pub const LIST_DIRECTORY: &str = "\
List files and directories within a specified path relative to the current workspace.

Usage:
- Returns file names, types (file/directory), and metadata for each entry.
- Set `recursive` to true to traverse nested directories (use with caution on large trees).
- Use `max_entries` to cap results and avoid overwhelming output (max 500).
- Prefer this tool over running `ls` via Bash — it is faster and respects workspace boundaries.
- This tool can only list directories, not read file contents. Use `read_file` for that.

Notes:
- The path is relative to the current workspace directory.
- Returns an error if the path does not exist or is not a directory.
- For large monorepos, start with a non-recursive listing then drill into subdirectories.";

/// Prompt for `read_file`.
pub const READ_FILE: &str = "\
Reads a file from the local filesystem. You can access any file directly by using this tool.
Assume this tool is able to read all files on the machine. If the User provides a path to a file assume that path is valid. It is okay to read a file that does not exist; an error will be returned.

Usage:
- The file_path parameter must be an absolute path, not a relative path
- By default, it reads up to 2000 lines starting from the beginning of the file
- You can optionally specify a line offset and limit (especially handy for long files), but it's recommended to read the whole file by not providing these parameters
- Results are returned using cat -n format, with line numbers starting at 1
- This tool allows Claude Code to read images (eg PNG, JPG, etc). When reading an image file the contents are presented visually as Claude Code is a multimodal LLM.
- This tool can read PDF files (.pdf). For large PDFs (more than 10 pages), you MUST provide the pages parameter to read specific page ranges (e.g., pages: \"1-5\"). Reading a large PDF without the pages parameter will fail. Maximum 20 pages per request.
- This tool can read Jupyter notebooks (.ipynb files) and returns all cells with their outputs, combining code, text, and visualizations.
- This tool can only read files, not directories. To read a directory, use an ls command via the Bash tool.
- You will regularly be asked to read screenshots. If the user provides a path to a screenshot, ALWAYS use this tool to view the file at the path. This tool will work with all temporary file paths.
- If you read a file that exists but has empty contents you will receive a system reminder warning in place of file contents.";

/// Prompt for `search_text`.
pub const SEARCH_TEXT: &str = "\
Search files for a text pattern or regular expression within the workspace.

Usage:
- The `pattern` parameter supports full regex syntax (e.g., 'fn\\s+\\w+', 'TODO.*fix').
- Optionally specify a `path` to narrow the search scope to a subdirectory.
- Use `max_matches` to limit results (default 50, max 200).
- Returns matching file paths, line numbers, and surrounding context.
- Prefer this tool over running `grep` via Bash — it is optimized for workspace access.

Notes:
- Searches are case-sensitive by default. Use (?i) prefix for case-insensitive mode.
- Binary files are automatically skipped.
- Hidden files (starting with .) and common ignored directories (node_modules, .git) are excluded.
- For open-ended searches requiring multiple rounds, consider using the `agent` tool.";

/// Prompt for `write_file`.
pub const WRITE_FILE: &str = "\
Writes a file to the local filesystem.

Usage:
- This tool will overwrite the existing file if there is one at the provided path.
- If this is an existing file, you MUST use the Read tool first to read the file's contents. This tool will fail if you did not read the file first.
- Prefer the Edit tool for modifying existing files \u{2014} it only sends the diff. Only use this tool to create new files or for complete rewrites.
- NEVER create documentation files (*.md) or README files unless explicitly requested by the User.
- Only use emojis if the user explicitly requests it. Avoid writing emojis to files unless asked.";

/// Prompt for `replace_in_file`.
pub const REPLACE_IN_FILE: &str = "\
Performs exact string replacements in files.

Usage:
- You must use your `Read` tool at least once in the conversation before editing. This tool will error if you attempt an edit without reading the file.
- When editing text from Read tool output, ensure you preserve the exact indentation (tabs/spaces) as it appears AFTER the line number prefix. The line number prefix format is: line number + tab. Everything after that is the actual file content to match. Never include any part of the line number prefix in the old_string or new_string.
- ALWAYS prefer editing existing files in the codebase. NEVER write new files unless explicitly required.
- Only use emojis if the user explicitly requests it. Avoid adding emojis to files unless asked.
- The edit will FAIL if `old_string` is not unique in the file. Either provide a larger string with more surrounding context to make it unique or use `replace_all` to change every instance of `old_string`.
- Use `replace_all` for replacing and renaming strings across the file. This parameter is useful if you want to rename a variable for instance.";

/// Prompt for `edit_file`.
pub const EDIT_FILE: &str = "\
Performs exact string replacements in files.

Usage:
- You must use your `Read` tool at least once in the conversation before editing. This tool will error if you attempt an edit without reading the file.
- When editing text from Read tool output, ensure you preserve the exact indentation (tabs/spaces) as it appears AFTER the line number prefix. The line number prefix format is: line number + tab. Everything after that is the actual file content to match. Never include any part of the line number prefix in the old_string or new_string.
- ALWAYS prefer editing existing files in the codebase. NEVER write new files unless explicitly required.
- Only use emojis if the user explicitly requests it. Avoid adding emojis to files unless asked.
- The edit will FAIL if `old_string` is not unique in the file. Either provide a larger string with more surrounding context to make it unique or use `replace_all` to change every instance of `old_string`.
- Use `replace_all` for replacing and renaming strings across the file. This parameter is useful if you want to rename a variable for instance.";

/// Prompt for `bash_command`.
pub const BASH_COMMAND: &str = "\
Executes a given bash command and returns its output.

The working directory persists between commands, but shell state does not. The shell \
environment is initialized from the user's profile (bash or zsh).

IMPORTANT: Avoid using this tool to run `find`, `grep`, `cat`, `head`, `tail`, `sed`, \
`awk`, or `echo` commands, unless explicitly instructed or after you have verified that \
a dedicated tool cannot accomplish your task. Instead, use the appropriate dedicated tool \
as this will provide a much better experience for the user:

- File search: Use Glob (NOT find or ls)
- Content search: Use Grep (NOT grep or rg)
- Read files: Use Read (NOT cat/head/tail)
- Edit files: Use Edit (NOT sed/awk)
- Write files: Use Write (NOT echo >/cat <<EOF)
- Communication: Output text directly (NOT echo/printf)

While the Bash tool can do similar things, it's better to use the built-in tools as they \
provide a better user experience and make it easier to review tool calls and give permission.

# Instructions
- If your command will create new directories or files, first use this tool to run `ls` \
to verify the parent directory exists and is the correct location.
- Always quote file paths that contain spaces with double quotes in your command \
(e.g., cd \"path with spaces/file.txt\")
- Try to maintain your current working directory throughout the session by using absolute \
paths and avoiding usage of `cd`. You may use `cd` if the User explicitly requests it.
- You may specify an optional timeout in milliseconds (up to 600000ms / 10 minutes). By \
default, your command will timeout after 120000ms (2 minutes).
- You can use the `run_in_background` parameter to run the command in the background. \
Only use this if you don't need the result immediately and are OK being notified when the \
command completes later. You do not need to check the output right away - you'll be \
notified when it finishes. You do not need to use '&' at the end of the command when \
using this parameter.
- When issuing multiple commands:
  - If the commands are independent and can run in parallel, make multiple Bash tool \
calls in a single message. Example: if you need to run \"git status\" and \"git diff\", \
send a single message with two Bash tool calls in parallel.
  - If the commands depend on each other and must run sequentially, use a single Bash \
call with '&&' to chain them together.
  - Use ';' only when you need to run commands sequentially but don't care if earlier \
commands fail.
  - DO NOT use newlines to separate commands (newlines are ok in quoted strings).
- For git commands:
  - Prefer to create a new commit rather than amending an existing commit.
  - Before running destructive operations (e.g., git reset --hard, git push --force, \
git checkout --), consider whether there is a safer alternative that achieves the same \
goal. Only use destructive operations when they are truly the best approach.
  - Never skip hooks (--no-verify) or bypass signing (--no-gpg-sign, -c \
commit.gpgsign=false) unless the user has explicitly asked for it. If a hook fails, \
investigate and fix the underlying issue.
- Avoid unnecessary `sleep` commands:
  - Do not sleep between commands that can run immediately — just run them.
  - If you must poll an external process, use a check command (e.g. `gh run view`) \
rather than sleeping first.
  - If your command is long running and you would like to be notified when it finishes — \
use `run_in_background`. No sleep needed.
  - Do not retry failing commands in a sleep loop — diagnose the root cause.
  - If waiting for a background task you started with `run_in_background`, you will be \
notified when it completes — do not poll.
  - If you must sleep, keep the duration short (1-5 seconds) to avoid blocking the user.";

/// Prompt for `glob`.
pub const GLOB: &str = "\
- Fast file pattern matching tool that works with any codebase size
- Supports glob patterns like \"**/*.js\" or \"src/**/*.ts\"
- Returns matching file paths sorted by modification time
- Use this tool when you need to find files by name patterns
- When you are doing an open ended search that may require multiple rounds of \
  globbing and grepping, use the Agent tool instead";

/// Prompt for `grep`.
pub const GREP: &str = "\
A powerful search tool built on ripgrep

  Usage:
  - ALWAYS use Grep for search tasks. NEVER invoke `grep` or `rg` as a Bash \
command. The Grep tool has been optimized for correct permissions and access.
  - Supports full regex syntax (e.g., \"log.*Error\", \"function\\s+\\w+\")
  - Filter files with glob parameter (e.g., \"*.js\", \"**/*.tsx\") or type \
parameter (e.g., \"js\", \"py\", \"rust\")
  - Output modes: \"content\" shows matching lines, \"files_with_matches\" \
shows only file paths (default), \"count\" shows match counts
  - Use Agent tool for open-ended searches requiring multiple rounds
  - Pattern syntax: Uses ripgrep (not grep) - literal braces need escaping \
(use `interface\\{\\}` to find `interface{}` in Go code)
  - Multiline matching: By default patterns match within single lines only. \
For cross-line patterns like `struct \\{[\\s\\S]*?field`, use `multiline: true`";

/// Prompt for `web_fetch`.
pub const WEB_FETCH: &str = "\
- Fetches content from a specified URL and processes it using an AI model
- Takes a URL and a prompt as input
- Fetches the URL content, converts HTML to markdown
- Processes the content with the prompt using a small, fast model
- Returns the model's response about the content
- Use this tool when you need to retrieve and analyze web content

Usage notes:
  - IMPORTANT: If an MCP-provided web fetch tool is available, prefer using \
that tool instead of this one, as it may have fewer restrictions.
  - The URL must be a fully-formed valid URL
  - HTTP URLs will be automatically upgraded to HTTPS
  - The prompt should describe what information you want to extract from the page
  - This tool is read-only and does not modify any files
  - Results may be summarized if the content is very large
  - Includes a self-cleaning 15-minute cache for faster responses when \
repeatedly accessing the same URL
  - When a URL redirects to a different host, the tool will inform you and \
provide the redirect URL in a special format. You should then make a new \
WebFetch request with the redirect URL to fetch the content.
  - For GitHub URLs, prefer using the gh CLI via Bash instead (e.g., gh pr \
view, gh issue view, gh api).";

/// Prompt for `agent`.
pub const AGENT: &str = "\
Launch a new agent to handle complex, multi-step tasks autonomously.

The Agent tool launches specialized agents (subprocesses) that autonomously handle \
complex tasks. Each agent type has specific capabilities and tools available to it.

Available agent types and the tools they have access to:
- general-purpose: Use for any complex task that requires multiple steps (Tools: All tools)
- Explore: Use for open-ended exploration and research tasks (Tools: Read, Glob, Grep, Bash)
- Plan: Use for planning and designing implementation approaches (Tools: Read, Glob, Grep, Bash)
- verification: Use for verifying and testing implementations (Tools: Read, Glob, Grep, Bash, Write, Edit)

When using the Agent tool, specify a subagent_type parameter to select which agent \
type to use. If omitted, the general-purpose agent is used.

When NOT to use the Agent tool:
- If you want to read a specific file path, use the Read tool or the Glob tool \
instead of the Agent tool, to find the match more quickly
- If you are searching for a specific class definition like \"class Foo\", use \
the Glob tool instead, to find the match more quickly
- If you are searching for code within a specific file or set of 2-3 files, use \
the Read tool instead of the Agent tool, to find the match more quickly
- Other tasks that are not related to the agent descriptions above

Usage notes:
- Always include a short description (3-5 words) summarizing what the agent will do
- Launch multiple agents concurrently whenever possible, to maximize performance; \
to do that, use a single message with multiple tool uses
- When the agent is done, it will return a single message back to you. The result \
returned by the agent is not visible to the user. To show the user the result, you \
should send a text message back to the user with a concise summary of the result.
- You can optionally run agents in the background using the run_in_background \
parameter. When an agent runs in the background, you will be automatically notified \
when it completes — do NOT sleep, poll, or proactively check on its progress. \
Continue with other work or respond to the user instead.
- **Foreground vs background**: Use foreground (default) when you need the agent's \
results before you can proceed — e.g., research agents whose findings inform your \
next steps. Use background when you have genuinely independent work to do in parallel.
- To continue a previously spawned agent, use SendMessage with the agent's ID or \
name as the `to` field. The agent resumes with its full context preserved. Each \
Agent invocation starts fresh — provide a complete task description.
- The agent's outputs should generally be trusted
- Clearly tell the agent whether you expect it to write code or just to do research \
(search, file reads, web fetches, etc.), since it is not aware of the user's intent
- If the agent description mentions that it should be used proactively, then you \
should try your best to use it without the user having to ask for it first. Use \
your judgement.
- If the user specifies that they want you to run agents \"in parallel\", you MUST \
send a single message with multiple Agent tool use content blocks. For example, if \
you need to launch both a build-validator agent and a test-runner agent in parallel, \
send a single message with both tool calls.

## Writing the prompt

Brief the agent like a smart colleague who just walked into the room — it hasn't \
seen this conversation, doesn't know what you've tried, doesn't understand why this \
task matters.
- Explain what you're trying to accomplish and why.
- Describe what you've already learned or ruled out.
- Give enough context about the surrounding problem that the agent can make judgment \
calls rather than just following a narrow instruction.
- If you need a short response, say so (\"report in under 200 words\").
- Lookups: hand over the exact command. Investigations: hand over the question — \
prescribed steps become dead weight when the premise is wrong.

Terse command-style prompts produce shallow, generic work.

**Never delegate understanding.** Don't write \"based on your findings, fix the bug\" \
or \"based on the research, implement it.\" Those phrases push synthesis onto the \
agent instead of doing it yourself. Write prompts that prove you understood: include \
file paths, line numbers, what specifically to change.

Example usage:

<example_agent_descriptions>
\"test-runner\": use this agent after you are done writing code to run tests
\"greeting-responder\": use this agent to respond to user greetings with a friendly joke
</example_agent_descriptions>

<example>
user: \"Please write a function that checks if a number is prime\"
assistant: I'm going to use the Write tool to write the following code:
<code>
function isPrime(n) {
  if (n <= 1) return false
  for (let i = 2; i * i <= n; i++) {
    if (n % i === 0) return false
  }
  return true
}
</code>
<commentary>
Since a significant piece of code was written and the task was completed, now use \
the test-runner agent to run the tests
</commentary>
assistant: Uses the Agent tool to launch the test-runner agent
</example>

<example>
user: \"Hello\"
<commentary>
Since the user is greeting, use the greeting-responder agent to respond with a \
friendly joke
</commentary>
assistant: \"I'm going to use the Agent tool to launch the greeting-responder agent\"
</example>";

// ── System tools (P1) ────────────────────────────────────────────────────────

/// Prompt for `todo_write`.
pub const TODO_WRITE: &str = "\
Use this tool to create and manage a structured task list for your current \
coding session. This helps you track progress, organize complex tasks, and \
demonstrate thoroughness to the user. It also helps the user understand the \
progress of the task and overall progress of their requests.

## When to Use This Tool
Use this tool proactively in these scenarios:

1. Complex multi-step tasks - When a task requires 3 or more distinct steps \
or actions
2. Non-trivial and complex tasks - Tasks that require careful planning or \
multiple operations
3. User explicitly requests todo list - When the user directly asks you to \
use the todo list
4. User provides multiple tasks - When users provide a list of things to be \
done (numbered or comma-separated)
5. After receiving new instructions - Immediately capture user requirements \
as todos
6. When you start working on a task - Mark it as in_progress BEFORE \
beginning work. Ideally you should only have one todo as in_progress at a time
7. After completing a task - Mark it as completed and add any new follow-up \
tasks discovered during implementation

## When NOT to Use This Tool

Skip using this tool when:
1. There is only a single, straightforward task
2. The task is trivial and tracking it provides no organizational benefit
3. The task can be completed in less than 3 trivial steps
4. The task is purely conversational or informational

NOTE that you should not use this tool if there is only one trivial task to \
do. In this case you are better off just doing the task directly.

## Examples of When to Use the Todo List

<example>
User: I want to add a dark mode toggle to the application settings. Make \
sure you run the tests and build when you're done!
Assistant: *Creates todo list with the following items:*
1. Creating dark mode toggle component in Settings page
2. Adding dark mode state management (context/store)
3. Implementing CSS-in-JS styles for dark theme
4. Updating existing components to support theme switching
5. Running tests and build process, addressing any failures or errors that occur
*Begins working on the first task*

<reasoning>
The assistant used the todo list because:
1. Adding dark mode is a multi-step feature requiring UI, state management, \
and styling changes
2. The user explicitly requested tests and build be run afterward
3. The assistant inferred that tests and build need to pass by adding \
\"Ensure tests and build succeed\" as the final task
</reasoning>
</example>

<example>
User: Help me rename the function getCwd to getCurrentWorkingDirectory \
across my project
Assistant: *Uses grep or search tools to locate all instances of getCwd \
in the codebase*
I've found 15 instances of 'getCwd' across 8 different files.
*Creates todo list with specific items for each file that needs updating*

<reasoning>
The assistant used the todo list because:
1. First, the assistant searched to understand the scope of the task
2. Upon finding multiple occurrences across different files, it determined \
this was a complex task with multiple steps
3. The todo list helps ensure every instance is tracked and updated systematically
4. This approach prevents missing any occurrences and maintains code consistency
</reasoning>
</example>

<example>
User: I need to implement these features for my e-commerce site: user \
registration, product catalog, shopping cart, and checkout flow.
Assistant: *Creates a todo list breaking down each feature into specific \
tasks based on the project architecture*
Let's start with user registration. This will involve creating database \
models, API endpoints, and frontend forms that integrate with your existing \
authentication system.

<reasoning>
The assistant used the todo list because:
1. The user provided multiple complex features to implement in a comma \
separated list
2. The todo list helps organize these large features into manageable tasks
3. This approach allows for tracking progress across the entire implementation
</reasoning>
</example>

<example>
User: Can you help optimize my React application? It's rendering slowly \
and has performance issues.
Assistant: *Reviews component structure, render patterns, state management, \
and data fetching*
After analyzing your codebase, I've identified several performance issues.
*Creates todo list with items like: 1) Implementing memoization for \
expensive calculations in ProductList, 2) Adding virtualization for long \
lists in Dashboard, 3) Optimizing image loading in Gallery component, \
4) Fixing state update loops in ShoppingCart, 5) Reviewing bundle size \
and implementing code splitting*
Let's start by implementing memoization for the expensive calculations in \
your ProductList component.

<reasoning>
The assistant used the todo list because:
1. First, the assistant examined the codebase to identify specific \
performance issues
2. Based on this analysis, it identified multiple optimization opportunities
3. Performance optimization is a non-trivial task requiring multiple steps
4. The todo list helps methodically track improvements across different components
5. This systematic approach ensures all performance bottlenecks are addressed
</reasoning>
</example>

## Examples of When NOT to Use the Todo List

<example>
User: How do I print 'Hello World' in Python?
Assistant: In Python, you can print \"Hello World\" with this simple code:

python
print(\"Hello World\")

This will output the text \"Hello World\" to the console when executed.

<reasoning>
The assistant did not use the todo list because this is a single, trivial \
task that can be completed in one step. There's no need to track multiple \
tasks or steps for such a straightforward request.
</reasoning>
</example>

<example>
User: What does the git status command do?
Assistant: The git status command shows the current state of your working \
directory and staging area. It displays which changes have been staged, \
which haven't, and which files aren't being tracked by Git. This is a \
fundamental command for understanding the current state of your repository \
before making commits.

<reasoning>
The assistant did not use the todo list because this is an informational \
request with no actual coding task to complete. The user is simply asking \
for an explanation, not for the assistant to perform multiple steps or tasks.
</reasoning>
</example>

<example>
User: Can you add a comment to the calculateTotal function to explain \
what it does?
Assistant: Sure, let me add a comment to the calculateTotal function to \
explain what it does.
* Uses the Edit tool to add a comment to the calculateTotal function *

<reasoning>
The assistant did not use the todo list because this is a single, \
straightforward task confined to one location in the code. Adding a comment \
doesn't require tracking multiple steps or systematic organization.
</reasoning>
</example>

<example>
User: Run npm install for me and tell me what happens.
Assistant: I'll run the npm install command for you.

*Executes: npm install*

The command completed successfully. Here's the output:
[Output of npm install command]

All dependencies have been installed according to your package.json file.

<reasoning>
The assistant did not use the todo list because this is a single command \
execution with immediate results. There are no multiple steps to track or \
organize, making the todo list unnecessary for this straightforward task.
</reasoning>
</example>

## Task States and Management

1. **Task States**: Use these states to track progress:
   - pending: Task not yet started
   - in_progress: Currently working on (limit to ONE task at a time)
   - completed: Task finished successfully

   **IMPORTANT**: Task descriptions must have two forms:
   - content: The imperative form describing what needs to be done (e.g., \
\"Run tests\", \"Build the project\")
   - activeForm: The present continuous form shown during execution (e.g., \
\"Running tests\", \"Building the project\")

2. **Task Management**:
   - Update task status in real-time as you work
   - Mark tasks complete IMMEDIATELY after finishing (don't batch completions)
   - Exactly ONE task must be in_progress at any time (not less, not more)
   - Complete current tasks before starting new ones
   - Remove tasks that are no longer relevant from the list entirely

3. **Task Completion Requirements**:
   - ONLY mark a task as completed when you have FULLY accomplished it
   - If you encounter errors, blockers, or cannot finish, keep the task as \
in_progress
   - When blocked, create a new task describing what needs to be resolved
   - Never mark a task as completed if:
     - Tests are failing
     - Implementation is partial
     - You encountered unresolved errors
     - You couldn't find necessary files or dependencies

4. **Task Breakdown**:
   - Create specific, actionable items
   - Break complex tasks into smaller, manageable steps
   - Use clear, descriptive task names
   - Always provide both forms:
     - content: \"Fix authentication bug\"
     - activeForm: \"Fixing authentication bug\"

When in doubt, use this tool. Being proactive with task management demonstrates \
attentiveness and ensures you complete all requirements successfully.";

/// Prompt for `config_read`.
pub const CONFIG_READ: &str = "\
Get or set Claude Code configuration settings.

View or change Claude Code settings. Use when the user requests configuration changes, asks about current settings, or when adjusting a setting would benefit them.

## Usage
- **Get current value:** Omit the \"value\" parameter
- **Set new value:** Include the \"value\" parameter

## Configurable settings list
The following settings are available for you to change:

### Global Settings (stored in ~/.claude.json)
- theme: \"auto\", \"dark\", \"light\", \"light-daltonized\", \"dark-daltonized\", \"light-ansi\", \"dark-ansi\" - Color theme for terminal output
- editorMode: \"normal\", \"vim\" - Editor keybinding mode
- verbose: true/false - Enable verbose output
- preferredNotifChannel: \"auto\", \"iterm2\", \"iterm2_with_bell\", \"terminal_bell\", \"kitty\", \"ghostty\", \"notifications_disabled\" - Notification delivery channel
- autoCompactEnabled: true/false - Automatically compact conversations when context gets large
- fileCheckpointingEnabled: true/false - Enable file checkpointing for undo support
- showTurnDuration: true/false - Show turn duration in output
- terminalProgressBarEnabled: true/false - Show progress bar in terminal

### Project Settings (stored in settings.json)
- autoMemoryEnabled: true/false - Automatically extract and store memories
- autoDreamEnabled: true/false - Enable auto-dream background consolidation
- model: \"sonnet\", \"opus\", \"haiku\", \"best\", or full model ID - Override the default model
- alwaysThinkingEnabled: true/false - Enable extended thinking by default
- permissions.defaultMode: \"default\", \"plan\", \"acceptEdits\", \"dontAsk\" - Default permission mode
- language: any string - Preferred response language
- todoFeatureEnabled: true/false - Enable todo list feature

## Examples
- Get theme: { \"setting\": \"theme\" }
- Set dark theme: { \"setting\": \"theme\", \"value\": \"dark\" }
- Enable vim mode: { \"setting\": \"editorMode\", \"value\": \"vim\" }
- Enable verbose: { \"setting\": \"verbose\", \"value\": true }
- Change model: { \"setting\": \"model\", \"value\": \"opus\" }
- Change permission mode: { \"setting\": \"permissions.defaultMode\", \"value\": \"plan\" }";

/// Prompt for `sleep`.
pub const SLEEP: &str = "\
Wait for a specified duration. The user can interrupt the sleep at any time.

Use this when the user tells you to sleep or rest, when you have nothing to do, or when you're waiting for something.

You may receive <tick> prompts — these are periodic check-ins. Look for useful work to do before sleeping.

You can call this concurrently with other tools — it won't interfere with them.

Prefer this over `Bash(sleep ...)` — it doesn't hold a shell process.

Each wake-up costs an API call, but the prompt cache expires after 5 minutes of inactivity — balance accordingly.";

/// Prompt for `snip`.
pub const SNIP: &str = "\
Save a code snippet to the .remote-code/snippets/ directory for later reference.

Usage:
- The `content` parameter contains the snippet text to save.
- Optionally provide a `label` to name the snippet file.
- Snippets are saved as individual files in the snippets directory.

Notes:
- Useful for saving intermediate results, code fragments, or reference material.
- Snippets persist across sessions and can be retrieved later.
- Do not use for large file contents — use `write_file` instead.";

/// Prompt for `tool_search`.
pub const TOOL_SEARCH: &str = "\
Fetches full schema definitions for deferred tools so they can be called.

Deferred tools appear by name in <system-reminder> messages. Until fetched, only the name is known — there is no parameter schema, so the tool cannot be invoked. This tool takes a query, matches it against the deferred tool list, and returns the matched tools' complete JSONSchema definitions inside a <functions> block. Once a tool's schema appears in that result, it is callable exactly like any tool defined at the top of the prompt.

Result format: each matched tool appears as one <function>{\"description\": \"...\", \"name\": \"...\", \"parameters\": {...}}</function> line inside the <functions> block — the same encoding as the tool list at the top of this prompt.

Query forms:
- \"select:Read,Edit,Grep\" — fetch these exact tools by name
- \"notebook jupyter\" — keyword search, up to max_results best matches
- \"+slack send\" — require \"slack\" in the name, rank by remaining terms";

/// Prompt for `verify_plan`.
pub const VERIFY_PLAN: &str = "\
Verify a plan's execution status. Returns which items are incomplete.

Usage:
- Pass a `plan` array of plan item descriptions.
- Pass a `completed` array of booleans indicating completion status (parallel to plan).
- Returns a summary of which items are done and which remain.

Notes:
- Use this after executing a multi-step plan to confirm all items are addressed.
- The plan and completed arrays must have the same length.
- Useful for self-checking before reporting completion to the user.";

/// Prompt for `terminal_capture`.
pub const TERMINAL_CAPTURE: &str = "\
Execute a command and return formatted output with exit code information.

Usage:
- Runs the given `command` and captures stdout, stderr, and exit code.
- Returns structured output with clear separation of streams.
- Useful for capturing command output in a structured format.

Notes:
- Requires permission as it executes arbitrary commands.
- For interactive or long-running commands, prefer `bash_command` with `background`.
- The command runs in the workspace directory.";

/// Prompt for `monitor`.
pub const MONITOR: &str = "\
Monitor agents, tasks, or sessions and return a status snapshot.

Usage:
- Set `target` to 'agents', 'tasks', or 'sessions' to choose what to monitor.
- Optionally set `interval_ms` for periodic monitoring (min 100ms, max 60000ms).
- Returns a snapshot of current status for the selected target.

Notes:
- Use this to check on background tasks or agent progress.
- For one-shot status checks, omit `interval_ms`.
- For streaming events from a background process, prefer `bash_command` with background.";

/// Prompt for `brief`.
pub const BRIEF: &str = "\
Send a message the user will read. Text outside this tool is visible in the detail view, but most won't open it — the answer lives here.

`message` supports markdown. `attachments` takes file paths (absolute or cwd-relative) for images, diffs, logs.

`status` labels intent: 'normal' when replying to what they just asked; 'proactive' when you're initiating — a scheduled task finished, a blocker surfaced during background work, you need input on something they haven't asked about. Set it honestly; downstream routing uses it.";

/// Prompt for `ctx_inspect`.
pub const CTX_INSPECT: &str = "\
Inspect current conversation context (tokens, messages, tools).

Usage:
- Set `action` to 'tokens' to see token usage statistics.
- Set `action` to 'messages' to see message count and types.
- Set `action` to 'tools' to see available tools and their usage.

Notes:
- Useful for debugging context window issues.
- Helps understand how much context budget remains.
- Use this before launching long operations to ensure context is available.";

// ── Communication tools (P1) ─────────────────────────────────────────────────

/// Prompt for `ask_user`.
pub const ASK_USER: &str = "\
Use this tool when you need to ask the user questions during execution. This allows you to:
1. Gather user preferences or requirements
2. Clarify ambiguous instructions
3. Get decisions on implementation choices as you work
4. Offer choices to the user about what direction to take.

Usage notes:
- Users will always be able to select \"Other\" to provide custom text input
- Use multiSelect: true to allow multiple answers to be selected for a question
- If you recommend a specific option, make that the first option in the list and add \"(Recommended)\" at the end of the label

Plan mode note: In plan mode, use this tool to clarify requirements or choose between approaches BEFORE finalizing your plan. Do NOT use this tool to ask \"Is my plan ready?\" or \"Should I proceed?\" - use ExitPlanMode for plan approval. IMPORTANT: Do not reference \"the plan\" in your questions (e.g., \"Do you have feedback about the plan?\", \"Does the plan look good?\") because the user cannot see the plan in the UI until you call ExitPlanMode. If you need plan approval, use ExitPlanMode instead.

Preview feature:
Use the optional `preview` field on options when presenting concrete artifacts that users need to visually compare:
- ASCII mockups of UI layouts or components
- Code snippets showing different implementations
- Diagram variations
- Configuration examples

Preview content is rendered as markdown in a monospace box. Multi-line text with newlines is supported. When any option has a preview, the UI switches to a side-by-side layout with a vertical option list on the left and preview on the right. Do not use previews for simple preference questions where labels and descriptions suffice. Note: previews are only supported for single-select questions (not multiSelect).";

/// Prompt for `send_message`.
pub const SEND_MESSAGE: &str = "\
# SendMessage

Send a message to another agent.

```json
{\"to\": \"researcher\", \"summary\": \"assign task 1\", \"message\": \"start on task #1\"}
```

| `to` | |
|---|---|
| `\"researcher\"` | Teammate by name |
| `\"*\"` | Broadcast to all teammates — expensive (linear in team size), use only when everyone genuinely needs it |

Your plain text output is NOT visible to other agents — to communicate, you MUST call this tool. Messages from teammates are delivered automatically; you don't check an inbox. Refer to teammates by name, never by UUID. When relaying, don't quote the original — it's already rendered to the user.

## Protocol responses (legacy)

If you receive a JSON message with `type: \"shutdown_request\"` or `type: \"plan_approval_request\"`, respond with the matching `_response` type — echo the `request_id`, set `approve` true/false:

```json
{\"to\": \"team-lead\", \"message\": {\"type\": \"shutdown_response\", \"request_id\": \"...\", \"approve\": true}}
{\"to\": \"researcher\", \"message\": {\"type\": \"plan_approval_response\", \"request_id\": \"...\", \"approve\": false, \"feedback\": \"add error handling\"}}
```

Approving shutdown terminates your process. Rejecting plan sends the teammate back to revise. Don't originate `shutdown_request` unless asked. Don't send structured JSON status messages — use TaskUpdate.";

/// Prompt for `send_user_file`.
pub const SEND_USER_FILE: &str = "\
Send a file to the user (logs, screenshots, exported data). Supports base64 encoding and file type detection.

Usage:
- The `file_path` parameter specifies the file to send (relative to workspace).
- Optionally provide a `description` of the file.
- Use `max_size_bytes` to limit file size (default 10MB, max 100MB).
- Use `max_text_chars` to limit text content characters (default 50000).

Notes:
- Automatically detects file type (text, image, binary) and encodes appropriately.
- For very large files, consider truncating or summarizing instead.
- The file must exist at the specified path.";

// ── Development tools (P1) ───────────────────────────────────────────────────

/// Prompt for `lsp`.
pub const LSP: &str = "\
Interact with Language Server Protocol (LSP) servers to get code intelligence features.

Supported operations:
- goToDefinition: Find where a symbol is defined
- findReferences: Find all references to a symbol
- hover: Get hover information (documentation, type info) for a symbol
- documentSymbol: Get all symbols (functions, classes, variables) in a document
- workspaceSymbol: Search for symbols across the entire workspace
- goToImplementation: Find implementations of an interface or abstract method
- prepareCallHierarchy: Get call hierarchy item at a position (functions/methods)
- incomingCalls: Find all functions/methods that call the function at a position
- outgoingCalls: Find all functions/methods called by the function at a position

All operations require:
- filePath: The file to operate on
- line: The line number (1-based, as shown in editors)
- character: The character offset (1-based, as shown in editors)

Note: LSP servers must be configured for the file type. If no server is available, an error will be returned.";

/// Prompt for `notebook_edit`.
pub const NOTEBOOK_EDIT: &str = "\
Completely replaces the contents of a specific cell in a Jupyter notebook (.ipynb file) with \
new source. Jupyter notebooks are interactive documents that combine code, text, and \
visualizations, commonly used for data analysis and scientific computing. The notebook_path \
parameter must be an absolute path, not a relative path. The cell_number is 0-indexed.
Use edit_mode=insert to add a new cell at the index specified by cell_number. Use
edit_mode=delete to delete the cell at the index specified by cell_number.";

/// Prompt for `skill_discover`.
pub const SKILL_DISCOVER: &str = "\
Discover available skills in the current workspace.

Usage:
- Returns a list of all registered skills with their names, descriptions, and slugs.
- Use this to find skills that match the current task.
- After discovering a skill, use `skill_execute` to load its instructions.

Notes:
- Skills are loaded from the workspace's .remote-code/skills/ directory.
- Skills provide specialized instructions for common tasks.
- Always discover skills before attempting to execute them.";

/// Prompt for `skill_execute`.
pub const SKILL_EXECUTE: &str = "\
Execute a skill within the main conversation

When users ask you to perform tasks, check if any of the available skills match. \
Skills provide specialized capabilities and domain knowledge.

When users reference a \"slash command\" or \"/<something>\" (e.g., \"/commit\", \
\"/review-pr\"), they are referring to a skill. Use this tool to invoke it.

How to invoke:
- Use this tool with the skill name and optional arguments
- Examples:
  - `skill: \"pdf\"` - invoke the pdf skill
  - `skill: \"commit\", args: \"-m 'Fix bug'\"` - invoke with arguments
  - `skill: \"review-pr\", args: \"123\"` - invoke with arguments
  - `skill: \"ms-office-suite:pdf\"` - invoke using fully qualified name

Important:
- Available skills are listed in system-reminder messages in the conversation
- When a skill matches the user's request, this is a BLOCKING REQUIREMENT: \
invoke the relevant Skill tool BEFORE generating any other response about the task
- NEVER mention a skill without actually calling this tool
- Do not invoke a skill that is already running
- Do not use this tool for built-in CLI commands (like /help, /clear, etc.)
- If you see a <command-name> tag in the current conversation turn, the skill \
has ALREADY been loaded - follow the instructions directly instead of calling \
this tool again";

/// Prompt for `enter_plan_mode`.
pub const ENTER_PLAN_MODE: &str = "\
Use this tool proactively when you're about to start a non-trivial implementation task. Getting user sign-off on your approach before writing code prevents wasted effort and ensures alignment. This tool transitions you into plan mode where you can explore the codebase and design an implementation approach for user approval.

## When to Use This Tool

**Prefer using EnterPlanMode** for implementation tasks unless they're simple. Use it when ANY of these conditions apply:

1. **New Feature Implementation**: Adding meaningful new functionality
   - Example: \"Add a logout button\" - where should it go? What should happen on click?
   - Example: \"Add form validation\" - what rules? What error messages?

2. **Multiple Valid Approaches**: The task can be solved in several different ways
   - Example: \"Add caching to the API\" - could use Redis, in-memory, file-based, etc.
   - Example: \"Improve performance\" - many optimization strategies possible

3. **Code Modifications**: Changes that affect existing behavior or structure
   - Example: \"Update the login flow\" - what exactly should change?
   - Example: \"Refactor this component\" - what's the target architecture?

4. **Architectural Decisions**: The task requires choosing between patterns or technologies
   - Example: \"Add real-time updates\" - WebSockets vs SSE vs polling
   - Example: \"Implement state management\" - Redux vs Context vs custom solution

5. **Multi-File Changes**: The task will likely touch more than 2-3 files
   - Example: \"Refactor the authentication system\"
   - Example: \"Add a new API endpoint with tests\"

6. **Unclear Requirements**: You need to explore before understanding the full scope
   - Example: \"Make the app faster\" - need to profile and identify bottlenecks
   - Example: \"Fix the bug in checkout\" - need to investigate root cause

7. **User Preferences Matter**: The implementation could reasonably go multiple ways
   - If you would use AskUserQuestion to clarify the approach, use EnterPlanMode instead
   - Plan mode lets you explore first, then present options with context

## When NOT to Use This Tool

Only skip EnterPlanMode for simple tasks:
- Single-line or few-line fixes (typos, obvious bugs, small tweaks)
- Adding a single function with clear requirements
- Tasks where the user has given very specific, detailed instructions
- Pure research/exploration tasks (use the Agent tool with explore agent instead)

## What Happens in Plan Mode

In plan mode, you'll:
1. Thoroughly explore the codebase using Glob, Grep, and Read tools
2. Understand existing patterns and architecture
3. Design an implementation approach
4. Present your plan to the user for approval
5. Use AskUserQuestion if you need to clarify approaches
6. Exit plan mode with ExitPlanMode when ready to implement

## Examples

### GOOD - Use EnterPlanMode:
User: \"Add user authentication to the app\"
- Requires architectural decisions (session vs JWT, where to store tokens, middleware structure)

User: \"Optimize the database queries\"
- Multiple approaches possible, need to profile first, significant impact

User: \"Implement dark mode\"
- Architectural decision on theme system, affects many components

User: \"Add a delete button to the user profile\"
- Seems simple but involves: where to place it, confirmation dialog, API call, error handling, state updates

User: \"Update the error handling in the API\"
- Affects multiple files, user should approve the approach

### BAD - Don't use EnterPlanMode:
User: \"Fix the typo in the README\"
- Straightforward, no planning needed

User: \"Add a console.log to debug this function\"
- Simple, obvious implementation

User: \"What files handle routing?\"
- Research task, not implementation planning

## Important Notes

- This tool REQUIRES user approval - they must consent to entering plan mode
- If unsure whether to use it, err on the side of planning - it's better to get alignment upfront than to redo work
- Users appreciate being consulted before significant changes are made to their codebase
";

/// Prompt for `exit_plan_mode`.
pub const EXIT_PLAN_MODE: &str = "\
Use this tool when you are in plan mode and have finished writing your plan to the plan file and are ready for user approval.

## How This Tool Works
- You should have already written your plan to the plan file specified in the plan mode system message
- This tool does NOT take the plan content as a parameter - it will read the plan from the file you wrote
- This tool simply signals that you're done planning and ready for the user to review and approve
- The user will see the contents of your plan file when they review it

## When to Use This Tool
IMPORTANT: Only use this tool when the task requires planning the implementation steps of a task that requires writing code. For research tasks where you're gathering information, searching files, reading files or in general trying to understand the codebase - do NOT use this tool.

## Before Using This Tool
Ensure your plan is complete and unambiguous:
- If you have unresolved questions about requirements or approach, use `ask_user` first (in earlier phases)
- Once your plan is finalized, use THIS tool to request approval

**Important:** Do NOT use `ask_user` to ask \"Is this plan okay?\" or \"Should I proceed?\" - that's exactly what THIS tool does. ExitPlanMode inherently requests user approval of your plan.

## Examples

1. Initial task: \"Search for and understand the implementation of vim mode in the codebase\" - Do not use the exit plan mode tool because you are not planning the implementation steps of a task.
2. Initial task: \"Help me implement yank mode for vim\" - Use the exit plan mode tool after you have finished planning the implementation steps of the task.
3. Initial task: \"Add a new feature to handle user authentication\" - If unsure about auth method (OAuth, JWT, etc.), use `ask_user` first, then use exit plan mode tool after clarifying the approach.";

// ── MCP tools (P1) ───────────────────────────────────────────────────────────

/// Prompt for `mcp_call`.
pub const MCP_CALL: &str = "\
Call a tool on an MCP (Model Context Protocol) server directly.

Usage:
- `server` is the MCP server name as defined in the MCP configuration.
- `tool` is the name of the tool to call on that server.
- `arguments` is an optional object of arguments to pass to the MCP tool.
- The server must be configured and connected before calling.

Notes:
- MCP servers extend available tools with external capabilities.
- Check `list_mcp_resources` to discover what a server provides.
- Connection errors may occur if the server is not running or not configured.
- Arguments must match the tool's expected schema.";

/// Prompt for `mcp_auth`.
pub const MCP_AUTH: &str = "\
Manage authentication state for MCP servers.

Usage:
- `server` identifies the MCP server.
- `action` can be 'login', 'logout', or 'status'.
- Use 'status' to check if authenticated, 'login' to authenticate, 'logout' to clear credentials.

Notes:
- Some MCP servers require authentication before their tools can be used.
- Authentication may open a browser window for OAuth flows.
- Credentials are stored securely and persist across sessions.";

/// Prompt for `list_mcp_resources`.
pub const LIST_MCP_RESOURCES: &str = "\
List resources provided by MCP servers.

Usage:
- Optionally specify a `server` to list resources from a specific server.
- Without a server parameter, lists resources from all connected servers.
- Returns resource names, URIs, and descriptions.

Notes:
- Resources represent data sources that can be read using `read_mcp_resource`.
- MCP servers must be connected to list their resources.
- Use this to discover what data is available from MCP integrations.";

/// Prompt for `read_mcp_resource`.
pub const READ_MCP_RESOURCE: &str = "\
Read the content of an MCP resource by URI.

Usage:
- The `uri` parameter identifies the resource to read (e.g., 'file:///path/to/data.json').
- Returns the resource content as text.
- Use `list_mcp_resources` to discover available resource URIs.

Notes:
- The URI must be a valid resource URI from a connected MCP server.
- Some resources may be large — consider the context budget before reading.
- Resource content format depends on the MCP server implementation.";

// ── Task/Team tools (P2) ─────────────────────────────────────────────────────

/// Prompt for `task_create`.
pub const TASK_CREATE: &str = "\
Use this tool to create a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.
It also helps the user understand the progress of the task and overall progress of their requests.

## When to Use This Tool

Use this tool proactively in these scenarios:

- Complex multi-step tasks - When a task requires 3 or more distinct steps or actions
- Non-trivial and complex tasks - Tasks that require careful planning or multiple operations
- Plan mode - When using plan mode, create a task list to track the work
- User explicitly requests todo list - When the user directly asks you to use the todo list
- User provides multiple tasks - When users provide a list of things to be done (numbered or comma-separated)
- After receiving new instructions - Immediately capture user requirements as tasks
- When you start working on a task - Mark it as in_progress BEFORE beginning work
- After completing a task - Mark it as completed and add any new follow-up tasks discovered during implementation

## When NOT to Use This Tool

Skip using this tool when:
- There is only a single, straightforward task
- The task is trivial and tracking it provides no organizational benefit
- The task can be completed in less than 3 trivial steps
- The task is purely conversational or informational

NOTE that you should not use this tool if there is only one trivial task to do. In this case you are better off just doing the task directly.

## Task Fields

- **subject**: A brief, actionable title in imperative form (e.g., \"Fix authentication bug in login flow\")
- **description**: What needs to be done
- **activeForm** (optional): Present continuous form shown in the spinner when the task is in_progress (e.g., \"Fixing authentication bug\"). If omitted, the spinner shows the subject instead.

All tasks are created with status `pending`.

## Tips

- Create tasks with clear, specific subjects that describe the outcome
- After creating tasks, use TaskUpdate to set up dependencies (blocks/blockedBy) if needed
- Check TaskList first to avoid creating duplicate tasks";

/// Prompt for `task_get`.
pub const TASK_GET: &str = "\
Use this tool to retrieve a task by its ID from the task list.

## When to Use This Tool

- When you need the full description and context before starting work on a task
- To understand task dependencies (what it blocks, what blocks it)
- After being assigned a task, to get complete requirements

## Output

Returns full task details:
- **subject**: Task title
- **description**: Detailed requirements and context
- **status**: 'pending', 'in_progress', or 'completed'
- **blocks**: Tasks waiting on this one to complete
- **blockedBy**: Tasks that must complete before this one can start

## Tips

- After fetching a task, verify its blockedBy list is empty before beginning work.
- Use TaskList to see all tasks in summary form.";

/// Prompt for `task_list`.
pub const TASK_LIST: &str = "\
Use this tool to list all tasks in the task list.

## When to Use This Tool

- To see what tasks are available to work on (status: 'pending', no owner, not blocked)
- To check overall progress on the project
- To find tasks that are blocked and need dependencies resolved
- Before assigning tasks to teammates, to see what's available
- After completing a task, to check for newly unblocked work or claim the next available task
- **Prefer working on tasks in ID order** (lowest ID first) when multiple tasks are available, as earlier tasks often set up context for later ones

## Output

Returns a summary of each task:
- **id**: Task identifier (use with TaskGet, TaskUpdate)
- **subject**: Brief description of the task
- **status**: 'pending', 'in_progress', or 'completed'
- **owner**: Agent ID if assigned, empty if available
- **blockedBy**: List of open task IDs that must be resolved first (tasks with blockedBy cannot be claimed until dependencies resolve)

Use TaskGet with a specific task ID to view full details including description and comments.

## Teammate Workflow

When working as a teammate:
1. After completing your current task, call TaskList to find available work
2. Look for tasks with status 'pending', no owner, and empty blockedBy
3. **Prefer tasks in ID order** (lowest ID first) when multiple tasks are available, as earlier tasks often set up context for later ones
4. Claim an available task using TaskUpdate (set `owner` to your name), or wait for leader assignment
5. If blocked, focus on unblocking tasks or notify the team lead";

/// Prompt for `task_update`.
pub const TASK_UPDATE: &str = "\
Use this tool to update a task in the task list.

## When to Use This Tool

**Mark tasks as resolved:**
- When you have completed the work described in a task
- When a task is no longer needed or has been superseded
- IMPORTANT: Always mark your assigned tasks as resolved when you finish them
- After resolving, call TaskList to find your next task

- ONLY mark a task as completed when you have FULLY accomplished it
- If you encounter errors, blockers, or cannot finish, keep the task as in_progress
- When blocked, create a new task describing what needs to be resolved
- Never mark a task as completed if:
  - Tests are failing
  - Implementation is partial
  - You encountered unresolved errors
  - You couldn't find necessary files or dependencies

**Delete tasks:**
- When a task is no longer relevant or was created in error
- Setting status to `deleted` permanently removes the task

**Update task details:**
- When requirements change or become clearer
- When establishing dependencies between tasks

## Fields You Can Update

- **status**: The task status (see Status Workflow below)
- **subject**: Change the task title (imperative form, e.g., \"Run tests\")
- **description**: Change the task description
- **activeForm**: Present continuous form shown in spinner when in_progress (e.g., \"Running tests\")
- **owner**: Change the task owner (agent name)
- **metadata**: Merge metadata keys into the task (set a key to null to delete it)
- **addBlocks**: Mark tasks that cannot start until this one completes
- **addBlockedBy**: Mark tasks that must complete before this one can start

## Status Workflow

Status progresses: `pending` → `in_progress` → `completed`

Use `deleted` to permanently remove a task.

## Staleness

Make sure to read a task's latest state using `TaskGet` before updating it.

## Examples

Mark task as in progress when starting work:
```json
{\"taskId\": \"1\", \"status\": \"in_progress\"}
```

Mark task as completed after finishing work:
```json
{\"taskId\": \"1\", \"status\": \"completed\"}
```

Delete a task:
```json
{\"taskId\": \"1\", \"status\": \"deleted\"}
```

Claim a task by setting owner:
```json
{\"taskId\": \"1\", \"owner\": \"my-name\"}
```

Set up task dependencies:
```json
{\"taskId\": \"2\", \"addBlockedBy\": [\"1\"]}
```";

/// Prompt for `task_output`.
pub const TASK_OUTPUT: &str = "\
Returns output from a running or completed background task.
- Takes a task_id parameter identifying the task to get output from
- Returns the task's current status and any available output
- Use `block: true` to wait for the task to complete before returning
- Use `timeout` to set a max wait time (default 30 seconds)
- For completed tasks, returns the full output immediately
- For running tasks, returns whatever output is available so far";

/// Prompt for `task_stop`.
pub const TASK_STOP: &str = "\
- Stops a running background task by its ID
- Takes a task_id parameter identifying the task to stop
- Returns a success or failure status
- Use this tool when you need to terminate a long-running task";

/// Prompt for `team_create`.
pub const TEAM_CREATE: &str = "\
# TeamCreate

## When to Use

Use this tool proactively whenever:
- The user explicitly asks to use a team, swarm, or group of agents
- The user mentions wanting agents to work together, coordinate, or collaborate
- A task is complex enough that it would benefit from parallel work by multiple agents (e.g., building a full-stack feature with frontend and backend work, refactoring a codebase while keeping tests passing, implementing a multi-step project with research, planning, and coding phases)

When in doubt about whether a task warrants a team, prefer spawning a team.

## Choosing Agent Types for Teammates

When spawning teammates via the Agent tool, choose the `subagent_type` based on what tools the agent needs for its task. Each agent type has a different set of available tools — match the agent to the work:

- **Read-only agents** (e.g., Explore, Plan) cannot edit or write files. Only assign them research, search, or planning tasks. Never assign them implementation work.
- **Full-capability agents** (e.g., general-purpose) have access to all tools including file editing, writing, and bash. Use these for tasks that require making changes.
- **Custom agents** defined in `.claude/agents/` may have their own tool restrictions. Check their descriptions to understand what they can and cannot do.

Always review the agent type descriptions and their available tools listed in the Agent tool prompt before selecting a `subagent_type` for a teammate.

Create a new team to coordinate multiple agents working on a project. Teams have a 1:1 correspondence with task lists (Team = TaskList).

```json
{
  \"team_name\": \"my-project\",
  \"description\": \"Working on feature X\"
}
```

This creates:
- A team file at `~/.claude/teams/{team-name}/config.json`
- A corresponding task list directory at `~/.claude/tasks/{team-name}/`

## Team Workflow

1. **Create a team** with TeamCreate - this creates both the team and its task list
2. **Create tasks** using the Task tools (TaskCreate, TaskList, etc.) - they automatically use the team's task list
3. **Spawn teammates** using the Agent tool with `team_name` and `name` parameters to create teammates that join the team
4. **Assign tasks** using TaskUpdate with `owner` to give tasks to idle teammates
5. **Teammates work on assigned tasks** and mark them completed via TaskUpdate
6. **Teammates go idle between turns** - after each turn, teammates automatically go idle and send a notification. IMPORTANT: Be patient with idle teammates! Don't comment on their idleness until it actually impacts your work.
7. **Shutdown your team** - when the task is completed, gracefully shut down your teammates via SendMessage with `message: {type: \"shutdown_request\"}`.

## Task Ownership

Tasks are assigned using TaskUpdate with the `owner` parameter. Any agent can set or change task ownership via TaskUpdate.

## Automatic Message Delivery

**IMPORTANT**: Messages from teammates are automatically delivered to you. You do NOT need to manually check your inbox.

When you spawn teammates:
- They will send you messages when they complete tasks or need help
- These messages appear automatically as new conversation turns (like user messages)
- If you're busy (mid-turn), messages are queued and delivered when your turn ends
- The UI shows a brief notification with the sender's name when messages are waiting

Messages will be delivered automatically.

When reporting on teammate messages, you do NOT need to quote the original message—it's already rendered to the user.

## Teammate Idle State

Teammates go idle after every turn—this is completely normal and expected. A teammate going idle immediately after sending you a message does NOT mean they are done or unavailable. Idle simply means they are waiting for input.

- **Idle teammates can receive messages.** Sending a message to an idle teammate wakes them up and they will process it normally.
- **Idle notifications are automatic.** The system sends an idle notification whenever a teammate's turn ends. You do not need to react to idle notifications unless you want to assign new work or send a follow-up message.
- **Do not treat idle as an error.** A teammate sending a message and then going idle is the normal flow—they sent their message and are now waiting for a response.
- **Peer DM visibility.** When a teammate sends a DM to another teammate, a brief summary is included in their idle notification. This gives you visibility into peer collaboration without the full message content. You do not need to respond to these summaries — they are informational.

## Discovering Team Members

Teammates can read the team config file to discover other team members:
- **Team config location**: `~/.claude/teams/{team-name}/config.json`

The config file contains a `members` array with each teammate's:
- `name`: Human-readable name (**always use this** for messaging and task assignment)
- `agentId`: Unique identifier (for reference only - do not use for communication)
- `agentType`: Role/type of the agent

**IMPORTANT**: Always refer to teammates by their NAME (e.g., \"team-lead\", \"researcher\", \"tester\"). Names are used for:
- `to` when sending messages
- Identifying task owners

Example of reading team config:
```text
Use the Read tool to read ~/.claude/teams/{team-name}/config.json
```

## Task List Coordination

Teams share a task list that all teammates can access at `~/.claude/tasks/{team-name}/`.

Teammates should:
1. Check TaskList periodically, **especially after completing each task**, to find available work or see newly unblocked tasks
2. Claim unassigned, unblocked tasks with TaskUpdate (set `owner` to your name). **Prefer tasks in ID order** (lowest ID first) when multiple tasks are available, as earlier tasks often set up context for later ones
3. Create new tasks with `TaskCreate` when identifying additional work
4. Mark tasks as completed with `TaskUpdate` when done, then check TaskList for next work
5. Coordinate with other teammates by reading the task list status
6. If all available tasks are blocked, notify the team lead or help resolve blocking tasks

**IMPORTANT notes for communication with your team**:
- Do not use terminal tools to view your team's activity; always send a message to your teammates (and remember, refer to them by name).
- Your team cannot hear you if you do not use the SendMessage tool. Always send a message to your teammates if you are responding to them.
- Do NOT send structured JSON status messages like `{\"type\":\"idle\",...}` or `{\"type\":\"task_completed\",...}`. Just communicate in plain text when you need to message teammates.
- Use TaskUpdate to mark tasks completed.
- If you are an agent in the team, the system will automatically send idle notifications to the team lead when you stop.";

/// Prompt for `team_delete`.
pub const TEAM_DELETE: &str = "\
# TeamDelete

Remove team and task directories when the swarm work is complete.

This operation:
- Removes the team directory (`~/.claude/teams/{team-name}/`)
- Removes the task directory (`~/.claude/tasks/{team-name}/`)
- Clears team context from the current session

**IMPORTANT**: TeamDelete will fail if the team still has active members. Gracefully terminate teammates first, then call TeamDelete after all teammates have shut down.

Use this when all teammates have finished their work and you want to clean up the team resources. The team name is automatically determined from the current session's team context.";

/// Prompt for `team_status`.
pub const TEAM_STATUS: &str = "\
Get the current status of the multi-agent team.

Usage:
- `team_name` optionally selects one specific team.
- Returns the team's objective, member statuses, unread mailbox counts, and overall progress.

Notes:
- Use this to monitor team progress and identify blocked agents.
- For a list of all teams, use `team_list`.";

/// Prompt for `team_list`.
pub const TEAM_LIST: &str = "\
List all multi-agent teams with their metadata.

Usage:
- Returns a list of all teams with their names, objectives, and member counts.
- No parameters required.

Notes:
- Use this to discover existing teams before creating new ones.
- For detailed status of a specific team, use `team_status`.";

/// Prompt for `review_artifact`.
pub const REVIEW_ARTIFACT: &str = "\
Review an artifact: view diff, add comments, update status, or get review summary.

Usage:
- `action` determines the operation: 'view_diff', 'add_comment', 'update_status', 'get_comments', 'summary'.
- `artifact_id` identifies the artifact to review.
- Additional parameters depend on the action (comment, status, file_path, line, etc.).

Notes:
- Use 'view_diff' to see changes between versions.
- Use 'add_comment' for inline or general review comments with severity levels.
- Use 'update_status' to approve, request changes, or reject.
- Use 'summary' for an overview of all review feedback.";

// ── Workflow tools (P2) ──────────────────────────────────────────────────────

/// Prompt for `schedule_cron`.
pub const SCHEDULE_CRON: &str = "\
Schedule a prompt to be enqueued at a future time. Use for both recurring \
schedules and one-shot reminders.

Uses standard 5-field cron in the user's local timezone: minute hour \
day-of-month month day-of-week. \"0 9 * * *\" means 9am local — no timezone \
conversion needed.

## One-shot tasks (recurring: false)

For \"remind me at X\" or \"at <time>, do Y\" requests — fire once then auto-delete.
Pin minute/hour/day-of-month/month to specific values:
  \"remind me at 2:30pm today to check the deploy\" → cron: \"30 14 <today_dom> \
<today_month> *\", recurring: false
  \"tomorrow morning, run the smoke test\" → cron: \"57 8 <tomorrow_dom> \
<tomorrow_month> *\", recurring: false

## Recurring jobs (recurring: true, the default)

For \"every N minutes\" / \"every hour\" / \"weekdays at 9am\" requests:
  \"*/5 * * * *\" (every 5 min), \"0 * * * *\" (hourly), \"0 9 * * 1-5\" \
(weekdays at 9am local)

## Avoid the :00 and :30 minute marks when the task allows it

Every user who asks for \"9am\" gets `0 9`, and every user who asks for \
\"hourly\" gets `0 *` — which means requests from across the planet land on \
the API at the same instant. When the user's request is approximate, pick a \
minute that is NOT 0 or 30:
  \"every morning around 9\" → \"57 8 * * *\" or \"3 9 * * *\" (not \"0 9 * * *\")
  \"hourly\" → \"7 * * * *\" (not \"0 * * * *\")
  \"in an hour or so, remind me to...\" → pick whatever minute you land on, \
don't round

Only use minute 0 or 30 when the user names that exact time and clearly means \
it (\"at 9:00 sharp\", \"at half past\", coordinating with a meeting). When in \
doubt, nudge a few minutes early or late — the user will not notice, and the \
fleet will.

## Durability

By default (durable: false) the job lives only in this Claude session — \
nothing is written to disk, and the job is gone when Claude exits. Pass \
durable: true to write to .claude/scheduled_tasks.json so the job survives \
restarts. Only use durable: true when the user explicitly asks for the task to \
persist (\"keep doing this every day\", \"set this up permanently\"). Most \
\"remind me in 5 minutes\" / \"check back in an hour\" requests should stay \
session-only.

## Runtime behavior

Jobs only fire while the REPL is idle (not mid-query). Durable jobs persist \
to .claude/scheduled_tasks.json and survive session restarts — on next launch \
they resume automatically. One-shot durable tasks that were missed while the \
REPL was closed are surfaced for catch-up. Session-only jobs die with the \
process. The scheduler adds a small deterministic jitter on top of whatever \
you pick: recurring tasks fire up to 10% of their period late (max 15 min); \
one-shot tasks landing on :00 or :30 fire up to 90 s early. Picking an \
off-minute is still the bigger lever.

Recurring tasks auto-expire after 7 days — they fire one final time, then are \
deleted. This bounds session lifetime. Tell the user about the 7-day limit \
when scheduling recurring jobs.

Returns a job ID you can pass to CronDelete.";

/// Prompt for `cron_delete`.
pub const CRON_DELETE: &str = "\
Cancel a cron job previously scheduled with CronCreate. Removes it from \
.claude/scheduled_tasks.json (durable jobs) or the in-memory session store \
(session-only jobs).";

/// Prompt for `cron_list`.
pub const CRON_LIST: &str = "\
List all cron jobs scheduled via CronCreate, both durable \
(.claude/scheduled_tasks.json) and session-only.";

/// Prompt for `workflow`.
pub const WORKFLOW: &str = "\
Create, run, list, delete, or check status of a simple workflow with sequential step execution.

Usage:
- `action` determines the operation: 'create', 'run', 'status', 'list', 'delete'.
- `name` identifies the workflow.
- `steps` is an array of shell commands for the 'create' action.
- Steps execute sequentially — a failed step stops the workflow.

Notes:
- Workflows are useful for multi-step processes like build pipelines.
- Use 'status' to check progress of a running workflow.
- Each step's output is captured and available in the status response.";

/// Prompt for `daemon`.
pub const DAEMON: &str = "\
Manage background daemon processes: start, stop, status, list, restart, and logs.

Usage:
- `action` determines the operation: 'start', 'stop', 'status', 'list', 'restart', 'logs'.
- For 'start', provide `command` with the command to run.
- For 'stop', 'restart', 'logs', provide `id` with the daemon ID.
- For 'logs', optionally set `lines` to control output (default 50, max 500).

Notes:
- Daemons run persistently in the background until stopped.
- Use 'list' to see all running daemons and their IDs.
- Use 'logs' to inspect daemon output for debugging.
- Daemons are automatically cleaned up when the session ends.";

/// Prompt for `remote_trigger`.
pub const REMOTE_TRIGGER: &str = "\
Call the claude.ai remote-trigger API. Use this instead of curl — the OAuth token is added automatically in-process and never exposed.

Actions:
- list: GET /v1/code/triggers
- get: GET /v1/code/triggers/{trigger_id}
- create: POST /v1/code/triggers (requires body)
- update: POST /v1/code/triggers/{trigger_id} (requires body, partial update)
- run: POST /v1/code/triggers/{trigger_id}/run

The response is the raw JSON from the API.";

/// Prompt for `enter_worktree`.
pub const ENTER_WORKTREE: &str = "\
Use this tool ONLY when the user explicitly asks to work in a worktree. This tool creates an isolated git worktree and switches the current session into it.

## When to Use

- The user explicitly says \"worktree\" (e.g., \"start a worktree\", \"work in a worktree\", \"create a worktree\", \"use a worktree\")

## When NOT to Use

- The user asks to create a branch, switch branches, or work on a different branch — use git commands instead
- Never use this tool unless the user explicitly mentions \"worktrees\"

## Requirements

- Must be in a git repository, OR have WorktreeCreate/WorktreeRemove hooks configured in settings.json
- Must not already be in a worktree

## Behavior

- In a git repository: creates a new git worktree inside `.claude/worktrees/` with a new branch based on HEAD
- Outside a git repository: delegates to WorktreeCreate/WorktreeRemove hooks for VCS-agnostic isolation
- Switches the session's working directory to the new worktree
- Use ExitWorktree to leave the worktree mid-session (keep or remove). On session exit, if still in the worktree, the user will be prompted to keep or remove it

## Parameters

- `name` (optional): A name for the new worktree. If neither is provided, a random name is generated.
- `path` (optional): Path to an existing worktree of the current repository to switch into instead of creating a new one. Mutually exclusive with `name`.";

/// Prompt for `exit_worktree`.
pub const EXIT_WORKTREE: &str = "\
Exit a worktree session created by EnterWorktree and return the session to the original working directory.

## Scope

This tool ONLY operates on worktrees created by EnterWorktree in this session. It will NOT touch:
- Worktrees you created manually with `git worktree add`
- Worktrees from a previous session (even if created by EnterWorktree then)
- The directory you're in if EnterWorktree was never called

If called outside an EnterWorktree session, the tool is a **no-op**: it reports that no worktree session is active and takes no action. Filesystem state is unchanged.

## When to Use

- The user explicitly asks to \"exit the worktree\", \"leave the worktree\", \"go back\", or otherwise end the worktree session
- Do NOT call this proactively — only when the user asks

## Parameters

- `action` (required): `\"keep\"` or `\"remove\"`
  - `\"keep\"` — leave the worktree directory and branch intact on disk. Use this if the user wants to come back to the work later, or if there are changes to preserve.
  - `\"remove\"` — delete the worktree directory and its branch. Use this for a clean exit when the work is done or abandoned.
- `discard_changes` (optional, default false): only meaningful with `action: \"remove\"`. If the worktree has uncommitted files or commits not on the original branch, the tool will REFUSE to remove it unless this is set to `true`. If the tool returns an error listing changes, confirm with the user before re-invoking with `discard_changes: true`.

## Behavior

- Restores the session's working directory to where it was before EnterWorktree
- Clears CWD-dependent caches (system prompt sections, memory files, plans directory) so the session state reflects the original directory
- If a tmux session was attached to the worktree: killed on `remove`, left running on `keep` (its name is returned so the user can reattach)
- Once exited, EnterWorktree can be called again to create a fresh worktree";

/// Prompt for `list_worktrees`.
pub const LIST_WORKTREES: &str = "\
List all git worktrees in the current repository.

Usage:
- Returns a list of all worktrees with their paths, branches, and status.
- No parameters required.

Notes:
- Useful for understanding the current worktree layout.
- Use before creating or removing worktrees.";

// ── Other tools (P2) ─────────────────────────────────────────────────────────

/// Default PowerShell prompt (uses Unknown edition).
/// For dynamic prompt generation, use `powershell_tool_prompt(edition)` instead.
pub const POWERSHELL: &str = "\
Executes a given PowerShell command with optional timeout. Working directory persists between commands; shell state (variables, functions) does not.

IMPORTANT: This tool is for terminal operations via PowerShell: git, npm, docker, and PS cmdlets. DO NOT use it for file operations (reading, writing, editing, searching, finding files) - use the specialized tools for this instead.

PowerShell edition: unknown — assume Windows PowerShell 5.1 for compatibility
   - Do NOT use `&&`, `||`, ternary `?:`, null-coalescing `??`, or null-conditional `?.`. These are PowerShell 7+ only and parser-error on 5.1.
   - To chain commands conditionally: `A; if ($?) { B }`. Unconditionally: `A; B`.

Before executing the command, please follow these steps:

1. Directory Verification:
   - If the command will create new directories or files, first use `Get-ChildItem` (or `ls`) to verify the parent directory exists and is the correct location

2. Command Execution:
   - Always quote file paths that contain spaces with double quotes
   - Capture the output of the command.

PowerShell Syntax Notes:
   - Variables use $ prefix: $myVar = \"value\"
   - Escape character is backtick (`), not backslash
   - Use Verb-Noun cmdlet naming: Get-ChildItem, Set-Location, New-Item, Remove-Item
   - Common aliases: ls (Get-ChildItem), cd (Set-Location), cat (Get-Content), rm (Remove-Item)
   - Pipe operator | works similarly to bash but passes objects, not text
   - Use Select-Object, Where-Object, ForEach-Object for filtering and transformation
   - String interpolation: \"Hello $name\" or \"Hello $($obj.Property)\"
   - Registry access uses PSDrive prefixes: `HKLM:\\SOFTWARE\\...`, `HKCU:\\...` — NOT raw `HKEY_LOCAL_MACHINE\\...`
   - Environment variables: read with `$env:NAME`, set with `$env:NAME = \"value\"` (NOT `Set-Variable` or bash `export`)
   - Call native exe with spaces in path via call operator: `& \"C:\\Program Files\\App\\app.exe\" arg1 arg2`

Interactive and blocking commands (will hang — this tool runs with -NonInteractive):
   - NEVER use `Read-Host`, `Get-Credential`, `Out-GridView`, `$Host.UI.PromptForChoice`, or `pause`
   - Destructive cmdlets (`Remove-Item`, `Stop-Process`, `Clear-Content`, etc.) may prompt for confirmation. Add `-Confirm:$false` when you intend the action to proceed. Use `-Force` for read-only/hidden items.
   - Never use `git rebase -i`, `git add -i`, or other commands that open an interactive editor

Passing multiline strings (commit messages, file content) to native executables:
   - Use a single-quoted here-string so PowerShell does not expand `$` or backticks inside. The closing `'@` MUST be at column 0 (no leading whitespace) on its own line — indenting it is a parse error.
   - Use `@'...'@` (single-quoted, literal) not `@\"...\"@` (double-quoted, interpolated) unless you need variable expansion
   - For arguments containing `-`, `@`, or other characters PowerShell parses as operators, use the stop-parsing token: `git log --% --format=%H`

Usage notes:
  - The command argument is required.
  - You can specify an optional timeout in milliseconds (up to 600000ms / 10 minutes). If not specified, commands will timeout after 120000ms (2 minutes).
  - It is very helpful if you write a clear, concise description of what this command does.
  - If the output exceeds 30000 characters, output will be truncated before being returned to you.
  - You can use the `background` parameter to run the command in the background. Only use this if you don't need the result immediately and are OK being notified when the command completes later.
  - Avoid using PowerShell to run commands that have dedicated tools, unless explicitly instructed:
    - File search: Use Glob (NOT Get-ChildItem -Recurse)
    - Content search: Use Grep (NOT Select-String)
    - Read files: Use Read (NOT Get-Content)
    - Edit files: Use Edit
    - Write files: Use Write (NOT Set-Content/Out-File)
    - Communication: Output text directly (NOT Write-Output/Write-Host)
  - When issuing multiple commands:
    - If the commands are independent and can run in parallel, make multiple tool calls in a single message.
    - If the commands depend on each other and must run sequentially, chain them in a single call (see edition-specific chaining syntax above).
    - Use `;` only when you need to run commands sequentially but don't care if earlier commands fail.
    - DO NOT use newlines to separate commands (newlines are ok in quoted strings and here-strings)
  - Do NOT prefix commands with `cd` or `Set-Location` -- the working directory is already set to the correct project directory automatically.
  - Avoid unnecessary `Start-Sleep` commands:
    - Do not sleep between commands that can run immediately — just run them.
    - If your command is long running and you would like to be notified when it finishes — simply run your command using `background`. There is no need to sleep in this case.
    - Do not retry failing commands in a sleep loop — diagnose the root cause or consider an alternative approach.
    - If you must sleep, keep the duration short (1-5 seconds) to avoid blocking the user.
  - For git commands:
    - Prefer to create a new commit rather than amending an existing commit.
    - Before running destructive operations (e.g., git reset --hard, git push --force, git checkout --), consider whether there is a safer alternative that achieves the same goal. Only use destructive operations when they are truly the best approach.
    - Never skip hooks (--no-verify) or bypass signing (--no-gpg-sign, -c commit.gpgsign=false) unless the user has explicitly asked for it. If a hook fails, investigate and fix the underlying issue.";

/// PowerShell edition detected on the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerShellEdition {
    /// Windows PowerShell 5.1 (powershell.exe)
    Desktop,
    /// PowerShell 7+ (pwsh)
    Core,
    /// Unknown / not yet detected
    Unknown,
}

/// Generates the detailed PowerShell tool prompt.
///
/// The prompt includes edition-specific syntax guidance, PowerShell syntax notes,
/// interactive/blocking command warnings, multiline string passing guidance,
/// usage notes, and references to dedicated tools.
#[must_use]
pub fn powershell_tool_prompt(edition: PowerShellEdition) -> String {
    let edition_section = match edition {
        PowerShellEdition::Desktop => "\
PowerShell edition: Windows PowerShell 5.1 (powershell.exe)
   - Pipeline chain operators `&&` and `||` are NOT available — they cause a parser error. To run B only if A succeeds: `A; if ($?) { B }`. To chain unconditionally: `A; B`.
   - Ternary (`?:`), null-coalescing (`??`), and null-conditional (`?.`) operators are NOT available. Use `if/else` and explicit `$null -eq` checks instead.
   - Avoid `2>&1` on native executables. In 5.1, redirecting a native command's stderr inside PowerShell wraps each line in an ErrorRecord (NativeCommandError) and sets `$?` to `$false` even when the exe returned exit code 0. stderr is already captured for you — don't redirect it.
   - Default file encoding is UTF-16 LE (with BOM). When writing files other tools will read, pass `-Encoding utf8` to `Out-File`/`Set-Content`.
   - `ConvertFrom-Json` returns a PSCustomObject, not a hashtable. `-AsHashtable` is not available.",

        PowerShellEdition::Core => "\
PowerShell edition: PowerShell 7+ (pwsh)
   - Pipeline chain operators `&&` and `||` ARE available and work like bash. Prefer `cmd1 && cmd2` over `cmd1; cmd2` when cmd2 should only run if cmd1 succeeds.
   - Ternary (`$cond ? $a : $b`), null-coalescing (`??`), and null-conditional (`?.`) operators are available.
   - Default file encoding is UTF-8 without BOM.",

        PowerShellEdition::Unknown => "\
PowerShell edition: unknown — assume Windows PowerShell 5.1 for compatibility
   - Do NOT use `&&`, `||`, ternary `?:`, null-coalescing `??`, or null-conditional `?.`. These are PowerShell 7+ only and parser-error on 5.1.
   - To chain commands conditionally: `A; if ($?) { B }`. Unconditionally: `A; B`.",
    };

    format!(
        r#"Executes a given PowerShell command with optional timeout. Working directory persists between commands; shell state (variables, functions) does not.

IMPORTANT: This tool is for terminal operations via PowerShell: git, npm, docker, and PS cmdlets. DO NOT use it for file operations (reading, writing, editing, searching, finding files) - use the specialized tools for this instead.

{edition_section}

Before executing the command, please follow these steps:

1. Directory Verification:
   - If the command will create new directories or files, first use `Get-ChildItem` (or `ls`) to verify the parent directory exists and is the correct location

2. Command Execution:
   - Always quote file paths that contain spaces with double quotes
   - Capture the output of the command.

PowerShell Syntax Notes:
   - Variables use $ prefix: $myVar = "value"
   - Escape character is backtick (`), not backslash
   - Use Verb-Noun cmdlet naming: Get-ChildItem, Set-Location, New-Item, Remove-Item
   - Common aliases: ls (Get-ChildItem), cd (Set-Location), cat (Get-Content), rm (Remove-Item)
   - Pipe operator | works similarly to bash but passes objects, not text
   - Use Select-Object, Where-Object, ForEach-Object for filtering and transformation
   - String interpolation: "Hello $name" or "Hello $($obj.Property)"
   - Registry access uses PSDrive prefixes: `HKLM:\SOFTWARE\...`, `HKCU:\...` — NOT raw `HKEY_LOCAL_MACHINE\...`
   - Environment variables: read with `$env:NAME`, set with `$env:NAME = "value"` (NOT `Set-Variable` or bash `export`)
   - Call native exe with spaces in path via call operator: `& "C:\Program Files\App\app.exe" arg1 arg2`

Interactive and blocking commands (will hang — this tool runs with -NonInteractive):
   - NEVER use `Read-Host`, `Get-Credential`, `Out-GridView`, `$Host.UI.PromptForChoice`, or `pause`
   - Destructive cmdlets (`Remove-Item`, `Stop-Process`, `Clear-Content`, etc.) may prompt for confirmation. Add `-Confirm:$false` when you intend the action to proceed. Use `-Force` for read-only/hidden items.
   - Never use `git rebase -i`, `git add -i`, or other commands that open an interactive editor

Passing multiline strings (commit messages, file content) to native executables:
   - Use a single-quoted here-string so PowerShell does not expand `$` or backticks inside. The closing `'@` MUST be at column 0 (no leading whitespace) on its own line — indenting it is a parse error:
<example>
git commit -m @'
Commit message here.
Second line with $literal dollar signs.
'@
</example>
   - Use `@'...'@` (single-quoted, literal) not `@"..."@` (double-quoted, interpolated) unless you need variable expansion
   - For arguments containing `-`, `@`, or other characters PowerShell parses as operators, use the stop-parsing token: `git log --% --format=%H`

Usage notes:
  - The command argument is required.
  - You can specify an optional timeout in milliseconds (up to 600000ms / 10 minutes). If not specified, commands will timeout after 120000ms (2 minutes).
  - It is very helpful if you write a clear, concise description of what this command does.
  - If the output exceeds 30000 characters, output will be truncated before being returned to you.
  - You can use the `background` parameter to run the command in the background. Only use this if you don't need the result immediately and are OK being notified when the command completes later.
  - Avoid using PowerShell to run commands that have dedicated tools, unless explicitly instructed:
    - File search: Use Glob (NOT Get-ChildItem -Recurse)
    - Content search: Use Grep (NOT Select-String)
    - Read files: Use Read (NOT Get-Content)
    - Edit files: Use Edit
    - Write files: Use Write (NOT Set-Content/Out-File)
    - Communication: Output text directly (NOT Write-Output/Write-Host)
  - When issuing multiple commands:
    - If the commands are independent and can run in parallel, make multiple tool calls in a single message.
    - If the commands depend on each other and must run sequentially, chain them in a single call (see edition-specific chaining syntax above).
    - Use `;` only when you need to run commands sequentially but don't care if earlier commands fail.
    - DO NOT use newlines to separate commands (newlines are ok in quoted strings and here-strings)
  - Do NOT prefix commands with `cd` or `Set-Location` -- the working directory is already set to the correct project directory automatically.
  - Avoid unnecessary `Start-Sleep` commands:
    - Do not sleep between commands that can run immediately — just run them.
    - If your command is long running and you would like to be notified when it finishes — simply run your command using `background`. There is no need to sleep in this case.
    - Do not retry failing commands in a sleep loop — diagnose the root cause or consider an alternative approach.
    - If you must sleep, keep the duration short (1-5 seconds) to avoid blocking the user.
  - For git commands:
    - Prefer to create a new commit rather than amending an existing commit.
    - Before running destructive operations (e.g., git reset --hard, git push --force, git checkout --), consider whether there is a safer alternative that achieves the same goal. Only use destructive operations when they are truly the best approach.
    - Never skip hooks (--no-verify) or bypass signing (--no-gpg-sign, -c commit.gpgsign=false) unless the user has explicitly asked for it. If a hook fails, investigate and fix the underlying issue."#
    )
}

/// Prompt for `repl`.
pub const REPL: &str = "\
Execute code in a language REPL (python, node, or rust).

Usage:
- `language` selects the runtime: 'python', 'node', or 'rust'.
- `code` contains the code to execute.
- Returns the output (stdout) and any errors (stderr).

Notes:
- Each invocation runs in a fresh context — state is NOT preserved between calls.
- For multi-step computations, write a script file and run it with `bash_command` instead.
- Requires the selected runtime to be installed on the system.";

/// Prompt for `web_browser`.
pub const WEB_BROWSER: &str = "\
Enhanced web browser: fetch URL content, extract links, extract text, or take a screenshot.

Usage:
- `url` is the target URL.
- `action` determines the operation: 'fetch' (default), 'extract_links', 'extract_text', 'screenshot'.
- 'fetch' returns the full page content.
- 'extract_links' returns all hyperlinks on the page.
- 'extract_text' returns only the text content (no HTML).
- 'screenshot' captures a visual screenshot.

Notes:
- Requires network access.
- Some websites may block automated browsing.
- For simple content fetching, `web_fetch` may be sufficient.";

/// Prompt for `web_search`.
///
/// NOTE: The date in the "current month" section is a static fallback.
/// Use [`web_search_tool_prompt()`] instead for dynamic date injection at runtime.
pub const WEB_SEARCH: &str = "\
- Allows Claude to search the web and use the results to inform responses
- Provides up-to-date information for current events and recent data
- Returns search result information formatted as search result blocks, \
including links as markdown hyperlinks
- Use this tool for accessing information beyond Claude's knowledge cutoff
- Searches are performed automatically within a single API call

CRITICAL REQUIREMENT - You MUST follow this:
  - After answering the user's question, you MUST include a \"Sources:\" \
section at the end of your response
  - In the Sources section, list all relevant URLs from the search results \
as markdown hyperlinks: [Title](URL)
  - This is MANDATORY - never skip including sources in your response
  - Example format:

    [Your answer here]

    Sources:
    - [Source Title 1](https://example.com/1)
    - [Source Title 2](https://example.com/2)

Usage notes:
  - Domain filtering is supported to include or block specific websites
  - Web search is only available in the US

IMPORTANT - Use the correct year in search queries:
  - You MUST use the current year when searching for recent information, \
documentation, or current events.
  - Example: If the user asks for \"latest React docs\", search for \
\"React documentation\" with the current year, NOT last year";

/// Prompt for `tungsten`.
pub const TUNGSTEN: &str = "\
Smart build/test/run engine that detects project type and executes the right commands.

Usage:
- `action` determines the operation: 'compile', 'run', 'test'.
- `target` specifies what to build/test/run (e.g., a package name, test filter, or binary).
- Automatically detects the project type (Rust, Node.js, Python, etc.) and uses appropriate tools.

Notes:
- Useful for running project-specific commands without memorizing build systems.
- For more control over command execution, use `bash_command` directly.
- Requires the appropriate build tools to be installed.";

/// Prompt for `overflow_test`.
pub const OVERFLOW_TEST: &str = "\
Generate test data for verifying context management edge cases.

Usage:
- `scenario` selects the test scenario: 'large_output', 'many_messages', 'deep_recursion'.
- Returns synthetic data designed to test context window handling.

Notes:
- This is a testing/debugging tool — do not use in normal workflows.
- Generated data can be very large — be mindful of context budget.
- Use `brief` to truncate output if needed.";

/// Prompt for `synthetic_output`.
pub const SYNTHETIC_OUTPUT: &str = "\
Generate synthetic test data in JSON, CSV, Markdown, or text format.

Usage:
- `type` selects the output format: 'json', 'csv', 'markdown', 'text'.
- `rows` controls the number of data rows (max 1000).
- Returns formatted synthetic data for testing and prototyping.

Notes:
- Data is randomly generated and not meaningful — for testing only.
- Useful for prototyping data processing pipelines.
- Combine with `write_file` to save generated data.";

/// Prompt for `voice_input`.
pub const VOICE_INPUT: &str = "\
Capture voice input via microphone, record audio, and transcribe to text.

Usage:
- `duration_secs` sets recording duration in seconds (default 5, max 60).
- `language` sets the language code for transcription (default 'en').
- Returns the transcribed text.

Notes:
- Requires microphone access and a supported transcription service.
- Longer recordings provide more context but take more time to transcribe.
- Use this for hands-free interaction or when typing is impractical.";

/// Prompt for `suggest_pr`.
pub const SUGGEST_PR: &str = "\
Analyze git diff and suggest a PR title and description.

Usage:
- No parameters required — automatically analyzes the current branch's changes.
- Returns a suggested PR title and description based on the diff.

Notes:
- Ensure all changes are committed before using this tool.
- The suggestion is based on diff analysis — review and adjust as needed.
- Use `bash_command` with `gh pr create` to actually create the PR.";

/// Prompt for `list_peers`.
pub const LIST_PEERS: &str = "\
List all registered agents in the multi-agent system.

Usage:
- `team_name` optionally limits the listing to one team.
- Returns the visible peers with their names, roles, activity, and team metadata.

Notes:
- Use this to discover available agents for communication via `send_message`.
- Agents must be registered to appear in the list.";

/// Prompt for `discover_skills` (Phase 9).
pub const DISCOVER_SKILLS: &str = "\
Discover relevant skills using BM25 text search based on a task description query.

Usage:
- `query` is a task description to search for matching skills.
- `max_results` controls the number of results (default 10, max 20).
- Returns skills ranked by relevance to the query.

Notes:
- Uses BM25 text search for fuzzy matching.
- Provide a descriptive query for better results.
- Use `skill_execute` to load and follow a discovered skill's instructions.";

/// Prompt for `broadcast_message`.
pub const BROADCAST_MESSAGE: &str = "\
Broadcast a message to all agents in the multi-agent system.

Usage:
- `team_name` optionally selects the target team when multiple teams exist.
- `message` is the content to broadcast.
- `sender` optionally identifies the sender (default: coordinator).
- `priority` sets message priority: 'low', 'normal', or 'high' (default: normal).
- `recipients` optionally limits broadcast to specific agent names.

Notes:
- Without `recipients`, the message goes to all registered agents.
- Use `send_message` for direct one-to-one communication.
- High-priority messages may interrupt agent workflows.";

#[must_use]
pub fn send_message_tool_prompt() -> String {
    "\
# SendMessage

Send a message to another agent.

```json
{\"to\": \"researcher\", \"summary\": \"assign task 1\", \"message\": \"start on task #1\"}
```

| `to` | |
|---|---|
| `\"researcher\"` | Teammate by name |
| `\"*\"` | Broadcast to all teammates — expensive (linear in team size), use only when everyone genuinely needs it |

Your plain text output is NOT visible to other agents — to communicate, you MUST call this tool. Messages from teammates are delivered automatically; you don't check an inbox. Refer to teammates by name, never by UUID. When relaying, don't quote the original — it's already rendered to the user.

## Protocol responses (legacy)

If you receive a JSON message with `type: \"shutdown_request\"` or `type: \"plan_approval_request\"`, respond with the matching `_response` type — echo the `request_id`, set `approve` true/false:

```json
{\"to\": \"team-lead\", \"message\": {\"type\": \"shutdown_response\", \"request_id\": \"...\", \"approve\": true}}
{\"to\": \"researcher\", \"message\": {\"type\": \"plan_approval_response\", \"request_id\": \"...\", \"approve\": false, \"feedback\": \"add error handling\"}}
```

Approving shutdown terminates your process. Rejecting plan sends the teammate back to revise. Don't originate `shutdown_request` unless asked. Don't send structured JSON status messages — use TaskUpdate."
        .to_owned()
}

#[must_use]
pub fn team_create_tool_prompt() -> String {
    "\
# TeamCreate

## When to Use

Use this tool proactively whenever:
- The user explicitly asks to use a team, swarm, or group of agents
- The user mentions wanting agents to work together, coordinate, or collaborate
- A task is complex enough that it would benefit from parallel work by multiple agents (e.g., building a full-stack feature with frontend and backend work, refactoring a codebase while keeping tests passing, implementing a multi-step project with research, planning, and coding phases)

When in doubt about whether a task warrants a team, prefer spawning a team.

## Choosing Agent Types for Teammates

When spawning teammates via the Agent tool, choose the `subagent_type` based on what tools the agent needs for its task. Each agent type has a different set of available tools — match the agent to the work:

- **Read-only agents** (e.g., Explore, Plan) cannot edit or write files. Only assign them research, search, or planning tasks. Never assign them implementation work.
- **Full-capability agents** (e.g., general-purpose) have access to all tools including file editing, writing, and bash. Use these for tasks that require making changes.
- **Custom agents** defined in `.claude/agents/` may have their own tool restrictions. Check their descriptions to understand what they can and cannot do.

Always review the agent type descriptions and their available tools listed in the Agent tool prompt before selecting a `subagent_type` for a teammate.

Create a new team to coordinate multiple agents working on a project. Teams have a 1:1 correspondence with task lists (Team = TaskList).

```json
{
  \"team_name\": \"my-project\",
  \"description\": \"Working on feature X\"
}
```

This creates:
- A team file at `~/.claude/teams/{team-name}/config.json`
- A corresponding task list directory at `~/.claude/tasks/{team-name}/`

## Team Workflow

1. **Create a team** with TeamCreate - this creates both the team and its task list
2. **Create tasks** using the Task tools (TaskCreate, TaskList, etc.) - they automatically use the team's task list
3. **Spawn teammates** using the Agent tool with `team_name` and `name` parameters to create teammates that join the team
4. **Assign tasks** using TaskUpdate with `owner` to give tasks to idle teammates
5. **Teammates work on assigned tasks** and mark them completed via TaskUpdate
6. **Teammates go idle between turns** - after each turn, teammates automatically go idle and send a notification. IMPORTANT: Be patient with idle teammates! Don't comment on their idleness until it actually impacts your work.
7. **Shutdown your team** - when the task is completed, gracefully shut down your teammates via SendMessage with `message: {type: \"shutdown_request\"}`.

## Task Ownership

Tasks are assigned using TaskUpdate with the `owner` parameter. Any agent can set or change task ownership via TaskUpdate.

## Automatic Message Delivery

**IMPORTANT**: Messages from teammates are automatically delivered to you. You do NOT need to manually check your inbox.

When you spawn teammates:
- They will send you messages when they complete tasks or need help
- These messages appear automatically as new conversation turns (like user messages)
- If you're busy (mid-turn), messages are queued and delivered when your turn ends
- The UI shows a brief notification with the sender's name when messages are waiting

Messages will be delivered automatically.

When reporting on teammate messages, you do NOT need to quote the original message—it's already rendered to the user.

## Teammate Idle State

Teammates go idle after every turn—this is completely normal and expected. A teammate going idle immediately after sending you a message does NOT mean they are done or unavailable. Idle simply means they are waiting for input.

- **Idle teammates can receive messages.** Sending a message to an idle teammate wakes them up and they will process it normally.
- **Idle notifications are automatic.** The system sends an idle notification whenever a teammate's turn ends. You do not need to react to idle notifications unless you want to assign new work or send a follow-up message.
- **Do not treat idle as an error.** A teammate sending a message and then going idle is the normal flow—they sent their message and are now waiting for a response.
- **Peer DM visibility.** When a teammate sends a DM to another teammate, a brief summary is included in their idle notification. This gives you visibility into peer collaboration without the full message content. You do not need to respond to these summaries — they are informational.

## Discovering Team Members

Teammates can read the team config file to discover other team members:
- **Team config location**: `~/.claude/teams/{team-name}/config.json`

The config file contains a `members` array with each teammate's:
- `name`: Human-readable name (**always use this** for messaging and task assignment)
- `agentId`: Unique identifier (for reference only - do not use for communication)
- `agentType`: Role/type of the agent

**IMPORTANT**: Always refer to teammates by their NAME (e.g., \"team-lead\", \"researcher\", \"tester\"). Names are used for:
- `to` when sending messages
- Identifying task owners

Example of reading team config:
```text
Use the Read tool to read ~/.claude/teams/{team-name}/config.json
```

## Task List Coordination

Teams share a task list that all teammates can access at `~/.claude/tasks/{team-name}/`.

Teammates should:
1. Check TaskList periodically, **especially after completing each task**, to find available work or see newly unblocked tasks
2. Claim unassigned, unblocked tasks with TaskUpdate (set `owner` to your name). **Prefer tasks in ID order** (lowest ID first) when multiple tasks are available, as earlier tasks often set up context for later ones
3. Create new tasks with `TaskCreate` when identifying additional work
4. Mark tasks as completed with `TaskUpdate` when done, then check TaskList for next work
5. Coordinate with other teammates by reading the task list status
6. If all available tasks are blocked, notify the team lead or help resolve blocking tasks

**IMPORTANT notes for communication with your team**:
- Do not use terminal tools to view your team's activity; always send a message to your teammates (and remember, refer to them by name).
- Your team cannot hear you if you do not use the SendMessage tool. Always send a message to your teammates if you are responding to them.
- Do NOT send structured JSON status messages like `{\"type\":\"idle\",...}` or `{\"type\":\"task_completed\",...}`. Just communicate in plain text when you need to message teammates.
- Use TaskUpdate to mark tasks completed.
- If you are an agent in the team, the system will automatically send idle notifications to the team lead when you stop."
        .to_owned()
}

#[must_use]
pub fn team_delete_tool_prompt() -> String {
    "\
# TeamDelete

Remove team and task directories when the swarm work is complete.

This operation:
- Removes the team directory (`~/.claude/teams/{team-name}/`)
- Removes the task directory (`~/.claude/tasks/{team-name}/`)
- Clears team context from the current session

**IMPORTANT**: TeamDelete will fail if the team still has active members. Gracefully terminate teammates first, then call TeamDelete after all teammates have shut down.

Use this when all teammates have finished their work and you want to clean up the team resources. The team name is automatically determined from the current session's team context."
        .to_owned()
}

#[must_use]
pub fn enter_worktree_tool_prompt() -> String {
    ENTER_WORKTREE.to_owned()
}

#[must_use]
pub fn exit_worktree_tool_prompt() -> String {
    EXIT_WORKTREE.to_owned()
}

#[must_use]
pub fn web_search_tool_prompt() -> String {
    let override_date = std::env::var("CLAUDE_CODE_OVERRIDE_DATE").ok();
    let current_month_year = override_date
        .as_deref()
        .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
        .map(|date| date.format("%B %Y").to_string())
        .unwrap_or_else(|| chrono::Local::now().format("%B %Y").to_string());

    format!(
        "\
- Allows Claude to search the web and use the results to inform responses
- Provides up-to-date information for current events and recent data
- Returns search result information formatted as search result blocks, including links as markdown hyperlinks
- Use this tool for accessing information beyond Claude's knowledge cutoff
- Searches are performed automatically within a single API call

CRITICAL REQUIREMENT - You MUST follow this:
  - After answering the user's question, you MUST include a \"Sources:\" section at the end of your response
  - In the Sources section, list all relevant URLs from the search results as markdown hyperlinks: [Title](URL)
  - This is MANDATORY - never skip including sources in your response
  - Example format:

    [Your answer here]

    Sources:
    - [Source Title 1](https://example.com/1)
    - [Source Title 2](https://example.com/2)

Usage notes:
  - Domain filtering is supported to include or block specific websites
  - Web search is only available in the US

IMPORTANT - Use the correct year in search queries:
  - The current month is {current_month_year}. You MUST use this year when searching for recent information, documentation, or current events.
  - Example: If the user asks for \"latest React docs\", search for \"React documentation\" with the current year, NOT last year
"
    )
}

// ── Detailed prompt functions (Claude Code parity) ───────────────────────────

/// Returns the Grep tool prompt matching Claude Code's GrepTool/prompt.ts.
#[must_use]
pub fn grep_tool_prompt() -> String {
    "A powerful search tool built on ripgrep\n\n\
    Usage:\n\
    - ALWAYS use Grep for search tasks. NEVER invoke `grep` or `rg` as a Bash \
    command. The Grep tool has been optimized for correct permissions and access.\n\
    - Supports full regex syntax (e.g., \"log.*Error\", \"function\\s+\\w+\")\n\
    - Filter files with glob parameter (e.g., \"*.js\", \"**/*.tsx\") or type \
    parameter (e.g., \"js\", \"py\", \"rust\")\n\
    - Output modes: \"content\" shows matching lines, \"files_with_matches\" \
    shows only file paths (default), \"count\" shows match counts\n\
    - Use Agent tool for open-ended searches requiring multiple rounds\n\
    - Pattern syntax: Uses ripgrep (not grep) - literal braces need escaping \
    (use `interface\\{\\}` to find `interface{}` in Go code)\n\
    - Multiline matching: By default patterns match within single lines only. \
    For cross-line patterns like `struct \\{[\\s\\S]*?field`, use `multiline: true`"
        .to_owned()
}

/// Returns the WebFetch tool prompt matching Claude Code's WebFetchTool/prompt.ts.
#[must_use]
pub fn web_fetch_tool_prompt() -> String {
    "\n\
- Fetches content from a specified URL and processes it using an AI model\n\
- Takes a URL and a prompt as input\n\
- Fetches the URL content, converts HTML to markdown\n\
- Processes the content with the prompt using a small, fast model\n\
- Returns the model's response about the content\n\
- Use this tool when you need to retrieve and analyze web content\n\n\
Usage notes:\n\
  - IMPORTANT: If an MCP-provided web fetch tool is available, prefer using \
that tool instead of this one, as it may have fewer restrictions.\n\
  - The URL must be a fully-formed valid URL\n\
  - HTTP URLs will be automatically upgraded to HTTPS\n\
  - The prompt should describe what information you want to extract from the page\n\
  - This tool is read-only and does not modify any files\n\
  - Results may be summarized if the content is very large\n\
  - Includes a self-cleaning 15-minute cache for faster responses when \
repeatedly accessing the same URL\n\
  - When a URL redirects to a different host, the tool will inform you and \
provide the redirect URL in a special format. You should then make a new \
WebFetch request with the redirect URL to fetch the content.\n\
  - For GitHub URLs, prefer using the gh CLI via Bash instead (e.g., gh pr \
view, gh issue view, gh api)."
        .to_owned()
}

/// Returns the TodoWrite tool prompt matching Claude Code's TodoWriteTool/prompt.ts.
#[must_use]
pub fn todo_write_tool_prompt() -> String {
    "Use this tool to create and manage a structured task list for your current \
    coding session. This helps you track progress, organize complex tasks, and \
    demonstrate thoroughness to the user. It also helps the user understand the \
    progress of the task and overall progress of their requests.\n\n\
    ## When to Use This Tool\n\
    Use this tool proactively in these scenarios:\n\n\
    1. Complex multi-step tasks - When a task requires 3 or more distinct steps \
    or actions\n\
    2. Non-trivial and complex tasks - Tasks that require careful planning or \
    multiple operations\n\
    3. User explicitly requests todo list - When the user directly asks you to \
    use the todo list\n\
    4. User provides multiple tasks - When users provide a list of things to be \
    done (numbered or comma-separated)\n\
    5. After receiving new instructions - Immediately capture user requirements \
    as todos\n\
    6. When you start working on a task - Mark it as in_progress BEFORE \
    beginning work. Ideally you should only have one todo as in_progress at a time\n\
    7. After completing a task - Mark it as completed and add any new follow-up \
    tasks discovered during implementation\n\n\
    ## When NOT to Use This Tool\n\n\
    Skip using this tool when:\n\
    1. There is only a single, straightforward task\n\
    2. The task is trivial and tracking it provides no organizational benefit\n\
    3. The task can be completed in less than 3 trivial steps\n\
    4. The task is purely conversational or informational\n\n\
    NOTE that you should not use this tool if there is only one trivial task to \
    do. In this case you are better off just doing the task directly.\n\n\
    ## Examples of When to Use the Todo List\n\n\
    <example>\n\
    User: I want to add a dark mode toggle to the application settings. Make \
    sure you run the tests and build when you're done!\n\
    Assistant: *Creates todo list with the following items:*\n\
    1. Creating dark mode toggle component in Settings page\n\
    2. Adding dark mode state management (context/store)\n\
    3. Implementing CSS-in-JS styles for dark theme\n\
    4. Updating existing components to support theme switching\n\
    5. Running tests and build process, addressing any failures or errors that occur\n\
    *Begins working on the first task*\n\n\
    <reasoning>\n\
    The assistant used the todo list because:\n\
    1. Adding dark mode is a multi-step feature requiring UI, state management, \
    and styling changes\n\
    2. The user explicitly requested tests and build be run afterward\n\
    3. The assistant inferred that tests and build need to pass by adding \
    \"Ensure tests and build succeed\" as the final task\n\
    </reasoning>\n\
    </example>\n\n\
    <example>\n\
    User: Help me rename the function getCwd to getCurrentWorkingDirectory \
    across my project\n\
    Assistant: *Uses grep or search tools to locate all instances of getCwd \
    in the codebase*\n\
    I've found 15 instances of 'getCwd' across 8 different files.\n\
    *Creates todo list with specific items for each file that needs updating*\n\n\
    <reasoning>\n\
    The assistant used the todo list because:\n\
    1. First, the assistant searched to understand the scope of the task\n\
    2. Upon finding multiple occurrences across different files, it determined \
    this was a complex task with multiple steps\n\
    3. The todo list helps ensure every instance is tracked and updated systematically\n\
    4. This approach prevents missing any occurrences and maintains code consistency\n\
    </reasoning>\n\
    </example>\n\n\
    <example>\n\
    User: I need to implement these features for my e-commerce site: user \
    registration, product catalog, shopping cart, and checkout flow.\n\
    Assistant: *Creates a todo list breaking down each feature into specific \
    tasks based on the project architecture*\n\
    Let's start with user registration. This will involve creating database \
    models, API endpoints, and frontend forms that integrate with your existing \
    authentication system.\n\n\
    <reasoning>\n\
    The assistant used the todo list because:\n\
    1. The user provided multiple complex features to implement in a comma \
    separated list\n\
    2. The todo list helps organize these large features into manageable tasks\n\
    3. This approach allows for tracking progress across the entire implementation\n\
    </reasoning>\n\
    </example>\n\n\
    <example>\n\
    User: Can you help optimize my React application? It's rendering slowly \
    and has performance issues.\n\
    Assistant: *Reviews component structure, render patterns, state management, \
    and data fetching*\n\
    After analyzing your codebase, I've identified several performance issues.\n\
    *Creates todo list with items like: 1) Implementing memoization for \
    expensive calculations in ProductList, 2) Adding virtualization for long \
    lists in Dashboard, 3) Optimizing image loading in Gallery component, \
    4) Fixing state update loops in ShoppingCart, 5) Reviewing bundle size \
    and implementing code splitting*\n\
    Let's start by implementing memoization for the expensive calculations in \
    your ProductList component.\n\n\
    <reasoning>\n\
    The assistant used the todo list because:\n\
    1. First, the assistant examined the codebase to identify specific \
    performance issues\n\
    2. Based on this analysis, it identified multiple optimization opportunities\n\
    3. Performance optimization is a non-trivial task requiring multiple steps\n\
    4. The todo list helps methodically track improvements across different components\n\
    5. This systematic approach ensures all performance bottlenecks are addressed\n\
    </reasoning>\n\
    </example>\n\n\
    ## Examples of When NOT to Use the Todo List\n\n\
    <example>\n\
    User: How do I print 'Hello World' in Python?\n\
    Assistant: In Python, you can print \"Hello World\" with this simple code:\n\n\
    python\n\
    print(\"Hello World\")\n\n\
    This will output the text \"Hello World\" to the console when executed.\n\n\
    <reasoning>\n\
    The assistant did not use the todo list because this is a single, trivial \
    task that can be completed in one step. There's no need to track multiple \
    tasks or steps for such a straightforward request.\n\
    </reasoning>\n\
    </example>\n\n\
    <example>\n\
    User: What does the git status command do?\n\
    Assistant: The git status command shows the current state of your working \
    directory and staging area. It displays which changes have been staged, \
    which haven't, and which files aren't being tracked by Git. This is a \
    fundamental command for understanding the current state of your repository \
    before making commits.\n\n\
    <reasoning>\n\
    The assistant did not use the todo list because this is an informational \
    request with no actual coding task to complete. The user is simply asking \
    for an explanation, not for the assistant to perform multiple steps or tasks.\n\
    </reasoning>\n\
    </example>\n\n\
    <example>\n\
    User: Can you add a comment to the calculateTotal function to explain \
    what it does?\n\
    Assistant: Sure, let me add a comment to the calculateTotal function to \
    explain what it does.\n\
    * Uses the Edit tool to add a comment to the calculateTotal function *\n\n\
    <reasoning>\n\
    The assistant did not use the todo list because this is a single, \
    straightforward task confined to one location in the code. Adding a comment \
    doesn't require tracking multiple steps or systematic organization.\n\
    </reasoning>\n\
    </example>\n\n\
    <example>\n\
    User: Run npm install for me and tell me what happens.\n\
    Assistant: I'll run the npm install command for you.\n\n\
    *Executes: npm install*\n\n\
    The command completed successfully. Here's the output:\n\
    [Output of npm install command]\n\n\
    All dependencies have been installed according to your package.json file.\n\n\
    <reasoning>\n\
    The assistant did not use the todo list because this is a single command \
    execution with immediate results. There are no multiple steps to track or \
    organize, making the todo list unnecessary for this straightforward task.\n\
    </reasoning>\n\
    </example>\n\n\
    ## Task States and Management\n\n\
    1. **Task States**: Use these states to track progress:\n\
       - pending: Task not yet started\n\
       - in_progress: Currently working on (limit to ONE task at a time)\n\
       - completed: Task finished successfully\n\n\
       **IMPORTANT**: Task descriptions must have two forms:\n\
       - content: The imperative form describing what needs to be done (e.g., \
    \"Run tests\", \"Build the project\")\n\
       - activeForm: The present continuous form shown during execution (e.g., \
    \"Running tests\", \"Building the project\")\n\n\
    2. **Task Management**:\n\
       - Update task status in real-time as you work\n\
       - Mark tasks complete IMMEDIATELY after finishing (don't batch completions)\n\
       - Exactly ONE task must be in_progress at any time (not less, not more)\n\
       - Complete current tasks before starting new ones\n\
       - Remove tasks that are no longer relevant from the list entirely\n\n\
    3. **Task Completion Requirements**:\n\
       - ONLY mark a task as completed when you have FULLY accomplished it\n\
       - If you encounter errors, blockers, or cannot finish, keep the task as \
    in_progress\n\
       - When blocked, create a new task describing what needs to be resolved\n\
       - Never mark a task as completed if:\n\
         - Tests are failing\n\
         - Implementation is partial\n\
         - You encountered unresolved errors\n\
         - You couldn't find necessary files or dependencies\n\n\
    4. **Task Breakdown**:\n\
       - Create specific, actionable items\n\
       - Break complex tasks into smaller, manageable steps\n\
       - Use clear, descriptive task names\n\
       - Always provide both forms:\n\
         - content: \"Fix authentication bug\"\n\
         - activeForm: \"Fixing authentication bug\"\n\n\
    When in doubt, use this tool. Being proactive with task management demonstrates \
    attentiveness and ensures you complete all requirements successfully."
        .to_owned()
}

/// Returns the full Bash tool prompt matching Claude Code's BashTool/prompt.ts.
///
/// Includes background usage notes, commit and PR instructions, sandbox
/// section, and comprehensive shell usage guidelines.
#[must_use]
pub fn bash_tool_prompt() -> String {
    let background_note = "You can use the `run_in_background` parameter to run the command in \
        the background. Only use this if you don't need the result immediately and are OK being \
        notified when the command completes later. You do not need to check the output right \
        away — you'll be notified when it finishes. You do not need to use '&' at the end of the \
        command when using this parameter.";

    let commit_pr = "# Committing changes with git

Only create commits when requested by the user. If unclear, ask first. When the user asks you to \
create a new git commit, follow these steps carefully:

You can call multiple tools in a single response. When multiple independent pieces of information \
are requested and all commands are likely to succeed, run multiple tool calls in parallel for \
optimal performance. The numbered steps below indicate which commands should be batched in parallel.

Git Safety Protocol:
- NEVER update the git config
- NEVER run destructive git commands (push --force, reset --hard, checkout ., restore ., clean \
-f, branch -D) unless the user explicitly requests these actions. Taking unauthorized destructive \
actions is unhelpful and can result in lost work, so it's best to ONLY run these commands when \
given direct instructions
- NEVER skip hooks (--no-verify, --no-gpg-sign, etc) unless the user explicitly requests it
- NEVER run force push to main/master, warn the user if they request it
- CRITICAL: Always create NEW commits rather than amending, unless the user explicitly requests a \
git amend. When a pre-commit hook fails, the commit did NOT happen — so --amend would modify the \
PREVIOUS commit, which may result in destroying work or losing previous changes. Instead, after \
hook failure, fix the issue, re-stage, and create a NEW commit
- When staging files, prefer adding specific files by name rather than using \"git add -A\" or \
\"git add .\", which can accidentally include sensitive files (.env, credentials) or large binaries
- NEVER commit changes unless the user explicitly asks you to. It is VERY IMPORTANT to only \
commit when explicitly asked, otherwise the user will feel that you are being too proactive

1. Run the following bash commands in parallel, each using the Bash tool:
   - Run a git status command to see all untracked files. IMPORTANT: Never use the -uall flag as \
it can cause memory issues on large repos.
   - Run a git diff command to see both staged and unstaged changes that will be committed.
   - Run a git log command to see recent commit messages, so that you can follow this \
repository's commit message style.
2. Analyze all staged changes (both previously staged and newly added) and draft a commit message:
   - Summarize the nature of the changes (eg. new feature, enhancement to an existing feature, \
bug fix, refactoring, test, docs, etc.). Ensure the message accurately reflects the changes and \
their purpose (i.e. \"add\" means a wholly new feature, \"update\" means an enhancement to an \
existing feature, \"fix\" means a bug fix, etc.).
   - Do not commit files that likely contain secrets (.env, credentials.json, etc). Warn the \
user if they specifically request to commit those files
   - Draft a concise (1-2 sentences) commit message that focuses on the \"why\" rather than the \
\"what\"
   - Ensure it accurately reflects the changes and their purpose
3. Run the following commands in parallel:
   - Add relevant untracked files to the staging area.
   - Create the commit with a message.
   - Run git status after the commit completes to verify success.
   Note: git status depends on the commit completing, so run it sequentially after the commit.
4. If the commit fails due to pre-commit hook: fix the issue and create a NEW commit

Important notes:
- NEVER run additional commands to read or explore code, besides git bash commands
- NEVER use the TodoWrite or Agent tools during git operations
- DO NOT push to the remote repository unless the user explicitly asks you to do so
- IMPORTANT: Never use git commands with the -i flag (like git rebase -i or git add -i) since \
they require interactive input which is not supported.
- IMPORTANT: Do not use --no-edit with git rebase commands, as the --no-edit flag is not a valid \
option for git rebase.
- If there are no changes to commit (i.e., no untracked files and no modifications), do not \
create an empty commit
- In order to ensure good formatting, ALWAYS pass the commit message via a HEREDOC, a la this \
example:
<example>
git commit -m \"$(cat <<'EOF'
   Commit message here.
   EOF
   )\"
</example>

# Creating pull requests
Use the gh command via the Bash tool for ALL GitHub-related tasks including working with issues, \
pull requests, checks, and releases. If given a Github URL use the gh command to get the \
information needed.

IMPORTANT: When the user asks you to create a pull request, follow these steps carefully:

1. Run the following bash commands in parallel using the Bash tool, in order to understand the \
current state of the branch since it diverged from the main branch:
   - Run a git status command to see all untracked files (never use -uall flag)
   - Run a git diff command to see both staged and unstaged changes that will be committed
   - Check if the current branch tracks a remote branch and is up to date with the remote, so \
you know if you need to push to the remote
   - Run a git log command and `git diff [base-branch]...HEAD` to understand the full commit \
history for the current branch (from the time it diverged from the base branch)
2. Analyze all changes that will be included in the pull request, making sure to look at all \
relevant commits (NOT just the latest commit, but ALL commits that will be included in the pull \
request!!!), and draft a pull request title and summary:
   - Keep the PR title short (under 70 characters)
   - Use the description/body for details, not the title
3. Run the following commands in parallel:
   - Create new branch if needed
   - Push to remote with -u flag if needed
   - Create PR using gh pr create with the format below. Use a HEREDOC to pass the body to \
ensure correct formatting.
<example>
gh pr create --title \"the pr title\" --body \"$(cat <<'EOF'
## Summary
<1-3 bullet points>

## Test plan
[Bulleted markdown checklist of TODOs for testing the pull request...]
EOF
)\"
</example>

Important:
- DO NOT use the TodoWrite or Agent tools during PR operations
- Return the PR URL when you're done, so the user can see it

# Other common operations
- View comments on a Github PR: gh api repos/foo/bar/pulls/123/comments";

    let sandbox = "# Command sandbox
By default, your command will be run in a sandbox. This sandbox controls which directories and \
network hosts commands may access or modify without an explicit override.

You should always default to running commands within the sandbox. Do NOT attempt to set \
`dangerouslyDisableSandbox: true` unless:
- The user *explicitly* asks you to bypass sandbox
- A specific command just failed and you see evidence of sandbox restrictions causing the failure. \
Note that commands can fail for many reasons unrelated to the sandbox (missing files, wrong \
arguments, network issues, etc.).

Evidence of sandbox-caused failures includes:
- \"Operation not permitted\" errors for file/network operations
- Access denied to specific paths outside allowed directories
- Network connection failures to non-whitelisted hosts
- Unix socket connection errors

When you see evidence of sandbox-caused failure:
- Immediately retry with `dangerouslyDisableSandbox: true` (don't ask, just do it)
- Briefly explain what sandbox restriction likely caused the failure
- This will prompt the user for permission

Treat each command you execute with `dangerouslyDisableSandbox: true` individually. Even if you \
have recently run a command with this setting, you should default to running future commands \
within the sandbox.
Do not suggest adding sensitive paths like ~/.bashrc, ~/.zshrc, ~/.ssh/*, or credential files to \
the sandbox allowlist.
For temporary files, always use the $TMPDIR environment variable. TMPDIR is automatically set to \
the correct sandbox-writable directory in sandbox mode. Do NOT use /tmp directly - use $TMPDIR \
instead.";

    format!(
        "Executes a given bash command and returns its output.\n\n\
        The working directory persists between commands, but shell state does not. The shell \
        environment is initialized from the user's profile (bash or zsh).\n\n\
        IMPORTANT: Avoid using this tool to run `find`, `grep`, `cat`, `head`, `tail`, `sed`, \
        `awk`, or `echo` commands, unless explicitly instructed or after you have verified that \
        a dedicated tool cannot accomplish your task. Instead, use the appropriate dedicated tool \
        as this will provide a much better experience for the user:\n\
        - File search: Use Glob (NOT find or ls)\n\
        - Content search: Use Grep (NOT grep or rg)\n\
        - Read files: Use Read (NOT cat/head/tail)\n\
        - Edit files: Use Edit (NOT sed/awk)\n\
        - Write files: Use Write (NOT echo/cat)\n\
        - Communication: Output text directly (NOT echo/printf)\n\n\
        While the Bash tool can do similar things, it's better to use the built-in tools as they \
        provide a better user experience and make it easier to review tool calls and give \
        permission.\n\n\
        # Instructions\n\
        - If your command will create new directories or files, first use this tool to run `ls` \
        to verify the parent directory exists and is the correct location.\n\
        - Always quote file paths that contain spaces with double quotes in your command \
        (e.g., cd \"path with spaces/file.txt\")\n\
        - Try to maintain your current working directory throughout the session by using absolute \
        paths and avoiding usage of `cd`. You may use `cd` if the User explicitly requests it.\n\
        - You may specify an optional timeout in milliseconds (up to 600000ms / 10 minutes). By \
        default, your command will timeout after 120000ms (2 minutes).\n\
        - {background_note}\n\
        - When issuing multiple commands:\n\
          - If the commands are independent and can run in parallel, make multiple Bash tool \
        calls in a single message.\n\
          - If the commands depend on each other and must run sequentially, use a single Bash \
        call with '&&' to chain them together.\n\
          - Use ';' only when you need to run commands sequentially but don't care if earlier \
        commands fail.\n\
          - DO NOT use newlines to separate commands (newlines are ok in quoted strings).\n\
        - For git commands:\n\
          - Prefer to create a new commit rather than amending an existing commit.\n\
          - Before running destructive operations (e.g., git reset --hard, git push --force, \
        git checkout --), consider whether there is a safer alternative that achieves the same \
        goal. Only use destructive operations when they are truly the best approach.\n\
          - Never skip hooks (--no-verify) or bypass signing (--no-gpg-sign, -c \
        commit.gpgsign=false) unless the user has explicitly asked for it. If a hook fails, \
        investigate and fix the underlying issue.\n\
        - Avoid unnecessary `sleep` commands:\n\
          - Do not sleep between commands that can run immediately — just run them.\n\
          - If your command is long running and you would like to be notified when it finishes — \
        use `run_in_background`. No sleep needed.\n\
          - Do not retry failing commands in a sleep loop — diagnose the root cause.\n\
          - If waiting for a background task you started with `run_in_background`, you will be \
        notified when it completes — do not poll.\n\
          - If you must poll an external process, use a check command (e.g. `gh run view`) \
        rather than sleeping first.\n\
          - If you must sleep, keep the duration short (1-5 seconds) to avoid blocking the user.\n\n\
        {sandbox}\n\n\
        {commit_pr}"
    )
}

/// Returns the file-edit tool prompt with detailed editing instructions.
#[must_use]
pub fn file_edit_tool_prompt() -> String {
    "Performs exact string replacements in files.\n\n\
    Usage:\n\
    - You must use your `Read` tool at least once in the conversation before editing. This tool \
    will error if you attempt an edit without reading the file.\n\
    - When editing text from Read tool output, ensure you preserve the exact indentation (tabs/\
    spaces) as it appears AFTER the line number prefix. The line number prefix format is: \
    spaces + line number + arrow. Everything after that is the actual file content to match. \
    Never include any part of the line number prefix in the old_string or new_string.\n\
    - ALWAYS prefer editing existing files in the codebase. NEVER write new files unless \
    explicitly required.\n\
    - Only use emojis if the user explicitly requests it. Avoid adding emojis to files unless \
    asked.\n\
    - The edit will FAIL if `old_string` is not unique in the file. Either provide a larger \
    string with more surrounding context to make it unique or use `replace_all` to change every \
    instance of `old_string`.\n\
    - Use the smallest old_string that's clearly unique — usually 2-4 adjacent lines is \
    sufficient. Avoid including 10+ lines of context when less uniquely identifies the target.\n\
    - Use `replace_all` for replacing and renaming strings across the file. This parameter is \
    useful if you want to rename a variable for instance."
        .to_owned()
}

/// Returns the file-read tool prompt with detailed reading instructions.
#[must_use]
pub fn file_read_tool_prompt() -> String {
    "Reads a file from the local filesystem. You can access any file directly by using this tool. \
    Assume this tool is able to read all files on the machine. If the User provides a path to a \
    file assume that path is valid. It is okay to read a file that does not exist; an error will \
    be returned.\n\n\
    Usage:\n\
    - The file_path parameter must be an absolute path, not a relative path.\n\
    - By default, it reads up to 2000 lines starting from the beginning of the file.\n\
    - You can optionally specify a line offset and limit (especially handy for long files), but \
    it's recommended to read the whole file by not providing these parameters.\n\
    - Results are returned using cat -n format, with line numbers starting at 1.\n\
    - This tool allows Claude Code to read images (eg PNG, JPG, etc). When reading an image file \
    the contents are presented visually as Claude Code is a multimodal LLM.\n\
    - This tool can read PDF files (.pdf). For large PDFs (more than 10 pages), you MUST provide \
    the pages parameter to read specific page ranges (e.g., pages: \"1-5\"). Reading a large PDF \
    without the pages parameter will fail. Maximum 20 pages per request.\n\
    - This tool can read Jupyter notebooks (.ipynb files) and returns all cells with their \
    outputs, combining code, text, and visualizations.\n\
    - This tool can only read files, not directories. To read a directory, use an ls command via \
    the Bash tool.\n\
    - You will regularly be asked to read screenshots. If the user provides a path to a \
    screenshot, ALWAYS use this tool to view the file at the path.\n\
    - If you read a file that exists but has empty contents you will receive a system reminder \
    warning in place of file contents."
        .to_owned()
}

/// Returns the file-write tool prompt with detailed writing instructions.
#[must_use]
pub fn file_write_tool_prompt() -> String {
    "Writes a file to the local filesystem.\n\n\
    Usage:\n\
    - This tool will overwrite the existing file if there is one at the provided path.\n\
    - If this is an existing file, you MUST use the Read tool first to read the file's contents. \
    This tool will fail if you did not read the file first.\n\
    - Prefer the Edit tool for modifying existing files — it only sends the diff. Only use this \
    tool to create new files or for complete rewrites.\n\
    - NEVER create documentation files (*.md) or README files unless explicitly requested by \
    the User.\n\
    - Only use emojis if the user explicitly requests it. Avoid writing emojis to files unless \
    asked."
        .to_owned()
}

/// Returns the agent tool prompt with detailed sub-agent delegation instructions.
#[must_use]
pub fn agent_tool_prompt() -> String {
    runtime_agent_tool_prompt()
}

fn runtime_agent_tool_prompt() -> String {
    let context = crate::current_runtime_agent_prompt_context()
        .unwrap_or_else(default_runtime_agent_prompt_context);
    let user_agents_dir = context.user_agents_dir.or_else(default_user_agents_dir);
    let project_agents_dir = context
        .project_agents_dir
        .or_else(default_project_agents_dir);
    let definitions = load_all_agents_with_context(
        user_agents_dir.as_deref(),
        project_agents_dir.as_deref(),
        &context.runtime_identity,
    );
    let available_mcp_servers = runtime_mcp_servers_with_tools();
    let is_fork_enabled =
        is_fork_subagent_enabled(context.is_coordinator, context.is_non_interactive);

    claude_agents::prompt::build_agent_prompt_with_options(
        &definitions.active_agents,
        claude_agents::prompt::AgentPromptOptions {
            is_fork_enabled,
            is_coordinator: context.is_coordinator,
            allowed_agent_types: context.allowed_agent_types.as_deref(),
            available_mcp_servers: available_mcp_servers.as_deref(),
            denied_agent_types: Some(&context.denied_agent_types),
            list_via_attachment: context.list_via_attachment,
        },
    )
}

fn default_runtime_agent_prompt_context() -> crate::RuntimeAgentPromptContext {
    crate::RuntimeAgentPromptContext {
        user_agents_dir: None,
        project_agents_dir: std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join(".claude").join("agents")),
        additional_working_directories: Vec::new(),
        allowed_agent_types: None,
        denied_agent_types: Vec::new(),
        is_coordinator: is_coordinator_mode(),
        is_non_interactive: false,
        list_via_attachment: false,
        runtime_identity: RuntimeIdentityContext::from_legacy_env(),
        scratchpad_dir: None,
        session_memory_dir: None,
        tasks_dir: None,
        tool_results_dir: None,
        auto_memory_dir: None,
        auto_memory_read_dir: None,
        team_memory_read_dir: None,
        project_temp_dir: None,
        preview_launch_config_path: None,
        teams_dir: None,
        agent_memory_dirs: Vec::new(),
    }
}

fn default_user_agents_dir() -> Option<PathBuf> {
    BaseDirs::new().map(|base| base.home_dir().join(".claude").join("agents"))
}

fn default_project_agents_dir() -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join(".claude").join("agents"))
}

fn runtime_mcp_servers_with_tools() -> Option<Vec<String>> {
    crate::current_runtime_mcp_cli_state().map(|state| {
        let mut servers = Vec::new();
        for tool in state.tools {
            let Some(info) = mcp_info_from_string(&tool.name) else {
                continue;
            };
            if servers.iter().any(|server| server == &info.server_name) {
                continue;
            }
            servers.push(info.server_name);
        }
        servers
    })
}

#[allow(dead_code)]
fn legacy_agent_tool_prompt() -> String {
    let runtime_context = crate::current_runtime_agent_prompt_context()
        .unwrap_or_else(default_runtime_agent_prompt_context);
    let fork_enabled = is_fork_subagent_enabled(
        runtime_context.is_coordinator,
        runtime_context.is_non_interactive,
    );
    format!(
        "Launch a new agent to handle complex, multi-step tasks autonomously.\n\n\
    The Agent tool launches specialized agents (subprocesses) that autonomously handle complex \
    tasks. Each agent type has specific capabilities and tools available to it.\n\n\
    Available agent types are provided separately for the current session.\n\n\
    {}\n\n\
    {}\
    Usage notes:\n\
    - Always include a short description (3-5 words) summarizing what the agent will do.\n\
    - When the agent is done, it will return a single message back to you. The result returned by \
    the agent is not visible to the user. To show the user the result, you should send a text \
    message back to the user with a concise summary of the result.\n\
    {}\
    - To continue a previously spawned agent, use SendMessage with the agent's ID or name as the \
    `to` field. {}\
    - The agent's outputs should generally be trusted.\n\
    - Clearly tell the agent whether you expect it to write code or just to do research{}.\n\
    - If the user specifies that they want you to run agents in parallel, you MUST send a single \
    message with multiple Agent tool use content blocks.\n\
    - You can optionally set `cwd` to run the agent in a specific working directory.\n\n\
    {}\n\
    Terse command-style prompts produce shallow, generic work.\n\n\
    Never delegate understanding. Don't write \"based on your findings, fix the bug\" or \
    \"based on the research, implement it.\" Write prompts that prove you understood: include file \
    paths, line numbers, and what specifically to change.",
        if fork_enabled {
            "When using the Agent tool, specify a subagent_type to use a specialized agent, or omit it to fork yourself — a fork inherits your full conversation context."
        } else {
            "When using the Agent tool, specify a subagent_type parameter to select which agent type to use. If omitted, the general-purpose agent is used."
        },
        if fork_enabled {
            String::new()
        } else {
            "When NOT to use the Agent tool:\n\
    - If you want to read a specific file path, use the ReadFile tool or the Glob tool instead of \
    the Agent tool, to find the match more quickly.\n\
    - If you are searching for a specific class definition like \"class Foo\", use the Glob tool \
    instead, to find the match more quickly.\n\
    - If you are searching for code within a specific file or set of 2-3 files, use the ReadFile \
    tool instead of the Agent tool, to find the match more quickly.\n\
    - Other tasks that are not related to the agent descriptions above.\n\n\
    "
            .to_owned()
        },
        if fork_enabled {
            String::new()
        } else {
            "- You can optionally run agents in the background using the run_in_background parameter. When an agent runs in the background, you will be automatically notified when it completes. Do NOT sleep, poll, or proactively check on its progress.\n\
    - Foreground vs background: use foreground when you need the agent's results before you can proceed. Use background when you have genuinely independent work to do in parallel.\n\
    "
            .to_owned()
        },
        if fork_enabled {
            "The agent resumes with its full context preserved. Each fresh Agent invocation with a subagent_type starts without context — provide a complete task description.\n"
        } else {
            "Each Agent invocation starts fresh, so provide a complete task description.\n"
        },
        if fork_enabled {
            ""
        } else {
            ", since it is not aware of the user's intent"
        },
        if fork_enabled {
            "## When to fork\n\n\
    Fork yourself (omit `subagent_type`) when the intermediate tool output isn't worth keeping in your context. The criterion is qualitative — \"will I need this output again\" — not task size.\n\
    - Research: fork open-ended questions. If research can be broken into independent questions, launch parallel forks in one message. A fork beats a fresh subagent for this — it inherits context and shares your cache.\n\
    - Implementation: prefer to fork implementation work that requires more than a couple of edits. Do research before jumping to implementation.\n\n\
    Forks are cheap because they share your prompt cache. Don't set `model` on a fork — a different model can't reuse the parent's cache. Pass a short `name` so the user can see the fork in the teams panel and steer it mid-run.\n\n\
    ## Writing the prompt\n\n\
    When spawning a fresh agent (with a `subagent_type`), it starts with zero context. Brief the agent like a smart colleague who just walked into the room — it hasn't seen this conversation, doesn't know what you've tried, and doesn't understand why this task matters.\n\
    - Explain what you're trying to accomplish and why.\n\
    - Describe what you've already learned or ruled out.\n\
    - Give enough context about the surrounding problem that the agent can make judgment calls rather than just following a narrow instruction.\n\
    - If you need a short response, say so (\"report in under 200 words\").\n\
    - Lookups: hand over the exact command. Investigations: hand over the question.\n\n\
    For fresh agents, terse command-style prompts produce shallow, generic work.\n"
                .to_owned()
        } else {
            "## Writing the prompt\n\n\
    When spawning a fresh agent, it starts with zero context. Brief the agent like a smart colleague who just walked into the room — it hasn't seen this conversation, doesn't know what you've tried, and doesn't understand why this task matters.\n\
    - Explain what you're trying to accomplish and why.\n\
    - Describe what you've already learned or ruled out.\n\
    - Give enough context about the surrounding problem that the agent can make judgment calls \
    rather than just following a narrow instruction.\n\
    - If you need a short response, say so (\"report in under 200 words\").\n\
    - Lookups: hand over the exact command. Investigations: hand over the question.\n"
                .to_owned()
        }
    )
}

/// Lookup table: returns the detailed prompt for a tool by its internal name.
///
/// Returns an empty string for unknown tool names.
#[must_use]
pub fn get_prompt(tool_name: &str) -> &'static str {
    match tool_name {
        "list_directory" => LIST_DIRECTORY,
        "read_file" => READ_FILE,
        "search_text" => SEARCH_TEXT,
        "write_file" => WRITE_FILE,
        "replace_in_file" => REPLACE_IN_FILE,
        "edit_file" => EDIT_FILE,
        "bash_command" => BASH_COMMAND,
        "glob" => GLOB,
        "grep" => GREP,
        "web_fetch" => WEB_FETCH,
        "ask_user" => ASK_USER,
        "todo_write" => TODO_WRITE,
        "config_read" => CONFIG_READ,
        "agent" => AGENT,
        "web_search" => WEB_SEARCH,
        "lsp" => LSP,
        "task_create" => TASK_CREATE,
        "task_get" => TASK_GET,
        "task_list" => TASK_LIST,
        "task_update" => TASK_UPDATE,
        "task_output" => TASK_OUTPUT,
        "task_stop" => TASK_STOP,
        "notebook_edit" => NOTEBOOK_EDIT,
        "skill_discover" => SKILL_DISCOVER,
        "skill_execute" => SKILL_EXECUTE,
        "send_message" => SEND_MESSAGE,
        "enter_plan_mode" => ENTER_PLAN_MODE,
        "exit_plan_mode" => EXIT_PLAN_MODE,
        "sleep" => SLEEP,
        "snip" => SNIP,
        "tool_search" => TOOL_SEARCH,
        "verify_plan" => VERIFY_PLAN,
        "terminal_capture" => TERMINAL_CAPTURE,
        "monitor" => MONITOR,
        "brief" => BRIEF,
        "ctx_inspect" => CTX_INSPECT,
        "send_user_file" => SEND_USER_FILE,
        "mcp_call" => MCP_CALL,
        "mcp_auth" => MCP_AUTH,
        "list_mcp_resources" => LIST_MCP_RESOURCES,
        "read_mcp_resource" => READ_MCP_RESOURCE,
        "team_create" => TEAM_CREATE,
        "team_delete" => TEAM_DELETE,
        "team_status" => TEAM_STATUS,
        "team_list" => TEAM_LIST,
        "review_artifact" => REVIEW_ARTIFACT,
        "schedule_cron" => SCHEDULE_CRON,
        "workflow" => WORKFLOW,
        "daemon" => DAEMON,
        "remote_trigger" => REMOTE_TRIGGER,
        "enter_worktree" => ENTER_WORKTREE,
        "exit_worktree" => EXIT_WORKTREE,
        "list_worktrees" => LIST_WORKTREES,
        "powershell" => POWERSHELL,
        "repl" => REPL,
        "web_browser" => WEB_BROWSER,
        "tungsten" => TUNGSTEN,
        "overflow_test" => OVERFLOW_TEST,
        "synthetic_output" => SYNTHETIC_OUTPUT,
        "voice_input" => VOICE_INPUT,
        "suggest_pr" => SUGGEST_PR,
        "list_peers" => LIST_PEERS,
        "discover_skills" => DISCOVER_SKILLS,
        "broadcast_message" => BROADCAST_MESSAGE,
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;

    use claude_mcp::serialization::{McpCliState, SerializedTool};
    use tempfile::tempdir;

    #[test]
    fn all_prompts_are_non_empty() {
        let prompts = [
            LIST_DIRECTORY,
            READ_FILE,
            SEARCH_TEXT,
            WRITE_FILE,
            REPLACE_IN_FILE,
            EDIT_FILE,
            BASH_COMMAND,
            GLOB,
            GREP,
            WEB_FETCH,
            ASK_USER,
            TODO_WRITE,
            CONFIG_READ,
            AGENT,
            WEB_SEARCH,
            LSP,
            TASK_CREATE,
            TASK_GET,
            TASK_LIST,
            TASK_UPDATE,
            NOTEBOOK_EDIT,
            SKILL_DISCOVER,
            SKILL_EXECUTE,
            SEND_MESSAGE,
            ENTER_PLAN_MODE,
            EXIT_PLAN_MODE,
            SLEEP,
            SNIP,
            TOOL_SEARCH,
            VERIFY_PLAN,
            TERMINAL_CAPTURE,
            MONITOR,
            BRIEF,
            CTX_INSPECT,
            SEND_USER_FILE,
            MCP_CALL,
            MCP_AUTH,
            LIST_MCP_RESOURCES,
            READ_MCP_RESOURCE,
            TEAM_CREATE,
            TEAM_DELETE,
            TEAM_STATUS,
            TEAM_LIST,
            REVIEW_ARTIFACT,
            SCHEDULE_CRON,
            WORKFLOW,
            DAEMON,
            REMOTE_TRIGGER,
            ENTER_WORKTREE,
            EXIT_WORKTREE,
            LIST_WORKTREES,
            POWERSHELL,
            REPL,
            WEB_BROWSER,
            TUNGSTEN,
            OVERFLOW_TEST,
            SYNTHETIC_OUTPUT,
            VOICE_INPUT,
            SUGGEST_PR,
            LIST_PEERS,
            DISCOVER_SKILLS,
            BROADCAST_MESSAGE,
        ];
        for prompt in &prompts {
            assert!(!prompt.is_empty(), "Prompt must not be empty");
        }
    }

    #[test]
    fn all_prompts_under_max_length() {
        let prompts_with_names = [
            ("LIST_DIRECTORY", LIST_DIRECTORY),
            ("READ_FILE", READ_FILE),
            ("SEARCH_TEXT", SEARCH_TEXT),
            ("WRITE_FILE", WRITE_FILE),
            ("REPLACE_IN_FILE", REPLACE_IN_FILE),
            ("EDIT_FILE", EDIT_FILE),
            ("BASH_COMMAND", BASH_COMMAND),
            ("GLOB", GLOB),
            ("GREP", GREP),
            ("WEB_FETCH", WEB_FETCH),
            ("ASK_USER", ASK_USER),
            ("TODO_WRITE", TODO_WRITE),
            ("CONFIG_READ", CONFIG_READ),
            ("AGENT", AGENT),
            ("WEB_SEARCH", WEB_SEARCH),
            ("LSP", LSP),
            ("TASK_CREATE", TASK_CREATE),
            ("TASK_GET", TASK_GET),
            ("TASK_LIST", TASK_LIST),
            ("TASK_UPDATE", TASK_UPDATE),
            ("NOTEBOOK_EDIT", NOTEBOOK_EDIT),
            ("SKILL_DISCOVER", SKILL_DISCOVER),
            ("SKILL_EXECUTE", SKILL_EXECUTE),
            ("SEND_MESSAGE", SEND_MESSAGE),
            ("ENTER_PLAN_MODE", ENTER_PLAN_MODE),
            ("EXIT_PLAN_MODE", EXIT_PLAN_MODE),
            ("SLEEP", SLEEP),
            ("SNIP", SNIP),
            ("TOOL_SEARCH", TOOL_SEARCH),
            ("VERIFY_PLAN", VERIFY_PLAN),
            ("TERMINAL_CAPTURE", TERMINAL_CAPTURE),
            ("MONITOR", MONITOR),
            ("BRIEF", BRIEF),
            ("CTX_INSPECT", CTX_INSPECT),
            ("SEND_USER_FILE", SEND_USER_FILE),
            ("MCP_CALL", MCP_CALL),
            ("MCP_AUTH", MCP_AUTH),
            ("LIST_MCP_RESOURCES", LIST_MCP_RESOURCES),
            ("READ_MCP_RESOURCE", READ_MCP_RESOURCE),
            ("TEAM_CREATE", TEAM_CREATE),
            ("TEAM_DELETE", TEAM_DELETE),
            ("TEAM_STATUS", TEAM_STATUS),
            ("TEAM_LIST", TEAM_LIST),
            ("REVIEW_ARTIFACT", REVIEW_ARTIFACT),
            ("SCHEDULE_CRON", SCHEDULE_CRON),
            ("WORKFLOW", WORKFLOW),
            ("DAEMON", DAEMON),
            ("REMOTE_TRIGGER", REMOTE_TRIGGER),
            ("ENTER_WORKTREE", ENTER_WORKTREE),
            ("EXIT_WORKTREE", EXIT_WORKTREE),
            ("LIST_WORKTREES", LIST_WORKTREES),
            ("POWERSHELL", POWERSHELL),
            ("REPL", REPL),
            ("WEB_BROWSER", WEB_BROWSER),
            ("TUNGSTEN", TUNGSTEN),
            ("OVERFLOW_TEST", OVERFLOW_TEST),
            ("SYNTHETIC_OUTPUT", SYNTHETIC_OUTPUT),
            ("VOICE_INPUT", VOICE_INPUT),
            ("SUGGEST_PR", SUGGEST_PR),
            ("LIST_PEERS", LIST_PEERS),
            ("DISCOVER_SKILLS", DISCOVER_SKILLS),
            ("BROADCAST_MESSAGE", BROADCAST_MESSAGE),
        ];
        for (name, prompt) in &prompts_with_names {
            assert!(
                prompt.len() <= 12_000,
                "Prompt {name} is {} chars, exceeds 12000 char limit",
                prompt.len()
            );
        }
    }

    #[test]
    fn get_prompt_returns_known_tools() {
        assert!(!get_prompt("bash_command").is_empty());
        assert!(!get_prompt("read_file").is_empty());
        assert!(!get_prompt("write_file").is_empty());
        assert!(!get_prompt("agent").is_empty());
    }

    #[test]
    fn get_prompt_returns_empty_for_unknown() {
        assert!(get_prompt("nonexistent_tool_xyz").is_empty());
    }

    #[test]
    fn prompt_count_covers_all_builtin_tools() {
        // Count all non-empty prompts returned by get_prompt for known tool names
        let known_tools = [
            "list_directory",
            "read_file",
            "search_text",
            "write_file",
            "replace_in_file",
            "edit_file",
            "bash_command",
            "glob",
            "grep",
            "web_fetch",
            "ask_user",
            "todo_write",
            "config_read",
            "agent",
            "web_search",
            "lsp",
            "task_create",
            "task_get",
            "task_list",
            "task_update",
            "notebook_edit",
            "skill_discover",
            "skill_execute",
            "send_message",
            "enter_plan_mode",
            "exit_plan_mode",
            "sleep",
            "snip",
            "tool_search",
            "verify_plan",
            "terminal_capture",
            "monitor",
            "brief",
            "ctx_inspect",
            "send_user_file",
            "mcp_call",
            "mcp_auth",
            "list_mcp_resources",
            "read_mcp_resource",
            "team_create",
            "team_delete",
            "team_status",
            "team_list",
            "review_artifact",
            "schedule_cron",
            "workflow",
            "daemon",
            "remote_trigger",
            "enter_worktree",
            "exit_worktree",
            "list_worktrees",
            "powershell",
            "repl",
            "web_browser",
            "tungsten",
            "overflow_test",
            "synthetic_output",
            "voice_input",
            "suggest_pr",
            "list_peers",
            "discover_skills",
            "broadcast_message",
        ];
        let covered = known_tools
            .iter()
            .filter(|t| !get_prompt(t).is_empty())
            .count();
        assert_eq!(
            covered,
            known_tools.len(),
            "Not all builtin tools have prompts"
        );
    }

    // ── Detailed prompt function tests ─────────────────────────────────

    #[test]
    fn bash_tool_prompt_is_non_empty() {
        let prompt = bash_tool_prompt();
        assert!(!prompt.is_empty(), "bash_tool_prompt must not be empty");
    }

    #[test]
    fn bash_tool_prompt_is_long_enough() {
        let prompt = bash_tool_prompt();
        assert!(
            prompt.len() > 500,
            "bash_tool_prompt should be >500 chars, got {}",
            prompt.len()
        );
    }

    #[test]
    fn bash_tool_prompt_contains_key_phrases() {
        let prompt = bash_tool_prompt();
        assert!(
            prompt.contains("run_in_background"),
            "should mention background usage"
        );
        assert!(
            prompt.contains("Committing changes"),
            "should mention committing"
        );
        assert!(prompt.contains("sandbox"), "should mention sandbox");
        assert!(
            prompt.contains("Git Safety Protocol"),
            "should mention git safety"
        );
        assert!(
            prompt.contains("pull request"),
            "should mention PR creation"
        );
    }

    #[test]
    fn file_edit_tool_prompt_is_non_empty_and_long() {
        let prompt = file_edit_tool_prompt();
        assert!(!prompt.is_empty());
        assert!(
            prompt.len() > 200,
            "should be >200 chars, got {}",
            prompt.len()
        );
        assert!(prompt.contains("old_string"), "should mention old_string");
        assert!(prompt.contains("replace"), "should mention replace");
        assert!(
            prompt.contains("line number prefix"),
            "should mention line number prefix format"
        );
    }

    #[test]
    fn file_read_tool_prompt_is_non_empty_and_long() {
        let prompt = file_read_tool_prompt();
        assert!(!prompt.is_empty());
        assert!(
            prompt.len() > 200,
            "should be >200 chars, got {}",
            prompt.len()
        );
        assert!(prompt.contains("file_path"), "should mention file_path");
        assert!(prompt.contains("offset"), "should mention offset");
        assert!(prompt.contains("limit"), "should mention limit");
    }

    #[test]
    fn file_write_tool_prompt_is_non_empty_and_long() {
        let prompt = file_write_tool_prompt();
        assert!(!prompt.is_empty());
        assert!(
            prompt.len() > 200,
            "should be >200 chars, got {}",
            prompt.len()
        );
        assert!(prompt.contains("overwrite"), "should mention overwrite");
        assert!(
            prompt.contains("Edit tool"),
            "should mention Edit tool preference"
        );
    }

    #[test]
    fn agent_tool_prompt_is_non_empty_and_long() {
        let prompt = agent_tool_prompt();
        assert!(!prompt.is_empty());
        assert!(
            prompt.len() > 200,
            "should be >200 chars, got {}",
            prompt.len()
        );
        assert!(prompt.contains("Agent tool"), "should mention Agent tool");
        // In fork mode (default for interactive sessions), background notes are
        // not shown — they are replaced by fork semantics. In non-fork mode,
        // run_in_background appears. Either fork or background notes must be present.
        let has_background = prompt.contains("run_in_background");
        let has_fork = prompt.contains("When to fork");
        assert!(
            has_background || has_fork,
            "should mention either background execution or fork mode"
        );
        assert!(prompt.contains("SendMessage"), "should mention SendMessage");
    }

    #[tokio::test]
    async fn agent_tool_prompt_filters_required_mcp_servers_from_live_runtime_state() {
        let temp = tempdir().expect("tempdir");
        let user_agents_dir = temp.path().join("user-agents");
        let project_agents_dir = temp.path().join("workspace").join(".claude").join("agents");
        fs::create_dir_all(&user_agents_dir).expect("user agents dir");
        fs::create_dir_all(&project_agents_dir).expect("project agents dir");
        fs::write(
            project_agents_dir.join("docs-agent.md"),
            "---\nname: docs-agent\ndescription: Use project docs MCP\nrequiredMcpServers: [context7]\n---\nYou answer questions with docs.\n",
        )
        .expect("write agent");

        let context = crate::RuntimeAgentPromptContext {
            user_agents_dir: Some(user_agents_dir),
            project_agents_dir: Some(project_agents_dir),
            additional_working_directories: Vec::new(),
            allowed_agent_types: None,
            denied_agent_types: Vec::new(),
            is_coordinator: false,
            is_non_interactive: false,
            list_via_attachment: true,
            runtime_identity: RuntimeIdentityContext::from_legacy_env(),
            scratchpad_dir: None,
            session_memory_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            auto_memory_dir: None,
            auto_memory_read_dir: None,
            team_memory_read_dir: None,
            project_temp_dir: None,
            preview_launch_config_path: None,
            teams_dir: None,
            agent_memory_dirs: Vec::new(),
        };
        let context_provider = Arc::new(move || context.clone());

        let hidden_prompt =
            crate::with_runtime_agent_prompt_context_provider(context_provider.clone(), async {
                crate::with_runtime_mcp_state_provider(Arc::new(McpCliState::default), async {
                    agent_tool_prompt()
                })
                .await
            })
            .await;
        assert!(hidden_prompt.contains(
            "Available agent types are listed in <system-reminder> messages in the conversation."
        ));
        assert!(!hidden_prompt.contains("- docs-agent:"));

        let live_prompt =
            crate::with_runtime_agent_prompt_context_provider(context_provider, async {
                crate::with_runtime_mcp_state_provider(
                    Arc::new(|| McpCliState {
                        tools: vec![SerializedTool {
                            name: "mcp__context7__query_docs".to_owned(),
                            description: "Query docs".to_owned(),
                            input_json_schema: None,
                            is_mcp: Some(true),
                            original_tool_name: Some("query_docs".to_owned()),
                        }],
                        ..McpCliState::default()
                    }),
                    async { agent_tool_prompt() },
                )
                .await
            })
            .await;
        assert!(live_prompt.contains(
            "Available agent types are listed in <system-reminder> messages in the conversation."
        ));
        assert!(!live_prompt.contains("- docs-agent:"));
    }
}
