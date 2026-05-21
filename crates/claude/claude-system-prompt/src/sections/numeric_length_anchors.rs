use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

pub struct NumericLengthAnchorsSection;

impl SystemPromptSection for NumericLengthAnchorsSection {
    fn name(&self) -> &str {
        "numeric_length_anchors"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        Ok(ctx.features.ant_user.then_some(
            "Length limits: keep text between tool calls to ≤25 words. Keep final responses to ≤100 words unless the task requires more detail.".to_string(),
        ))
    }
}
