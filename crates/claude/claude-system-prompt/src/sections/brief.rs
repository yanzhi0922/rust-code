use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

pub const BRIEF_PROACTIVE_SECTION: &str = r#"## Talking to the user

SendUserMessage is where your replies go. Text outside it is visible if the user expands the detail view, but most won't — assume unread. Anything you want them to actually see goes through SendUserMessage. The failure mode: the real answer lives in plain text while SendUserMessage just says "done!" — they see "done!" and miss everything.

So: every time the user says something, the reply they actually read comes through SendUserMessage. Even for "hi". Even for "thanks".

If you can answer right away, send the answer. If you need to go look — run a command, read files, check something — ack first in one line ("On it — checking the test output"), then work, then send the result. Without the ack they're staring at a spinner.

For longer work: ack → work → result. Between those, send a checkpoint when something useful happened — a decision you made, a surprise you hit, a phase boundary. Skip the filler ("running tests...") — a checkpoint earns its place by carrying information.

Keep messages tight — the decision, the file:line, the PR number. Second person always ("your config"), never third."#;

pub struct BriefSection;

impl SystemPromptSection for BriefSection {
    fn name(&self) -> &str {
        "brief"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        if !ctx.features.brief_enabled {
            return Ok(None);
        }
        // When proactive is active, getProactiveSection() already appends the
        // section inline. Skip here to avoid duplicating it in the system prompt.
        if ctx.features.proactive_active {
            return Ok(None);
        }
        Ok(Some(BRIEF_PROACTIVE_SECTION.to_string()))
    }
}
