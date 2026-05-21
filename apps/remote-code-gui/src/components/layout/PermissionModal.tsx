import { useEffect, useMemo, useState } from 'react';
import { ShieldAlert } from 'lucide-react';
import { formatSensitivePath, redactSensitivePathsForDisplay } from '../../lib/utils';
import { useAppStore } from '../../stores/useAppStore';

const PROMPT_PREFIX = 'prompt:';

type AllowedPrompt = {
  tool: string;
  prompt: string;
};

type CodexQuestion = {
  id: string;
  header?: string;
  question?: string;
  options?: { label: string; description?: string }[] | null;
};

function formatInput(input: unknown): string {
  try {
    return JSON.stringify(input, null, 2);
  } catch {
    return String(input);
  }
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function stringField(record: Record<string, unknown> | null, ...keys: string[]): string | null {
  for (const key of keys) {
    const value = record?.[key];
    if (typeof value === 'string' && value.trim().length > 0) {
      return value;
    }
  }
  return null;
}

function extractAllowedPrompts(input: unknown): AllowedPrompt[] {
  const record = asRecord(input);
  const rawPrompts = record?.allowedPrompts;
  if (!Array.isArray(rawPrompts)) return [];

  return rawPrompts
    .map((item) => {
      const prompt = asRecord(item);
      const tool = typeof prompt?.tool === 'string' ? prompt.tool.trim() : '';
      const description = typeof prompt?.prompt === 'string' ? prompt.prompt.trim() : '';
      if (!tool || !description) return null;
      return { tool, prompt: description };
    })
    .filter((item): item is AllowedPrompt => Boolean(item));
}

function buildExitPlanPermissionUpdates(allowedPrompts: AllowedPrompt[]): unknown[] | undefined {
  if (allowedPrompts.length === 0) {
    return undefined;
  }

  return [
    {
      type: 'addRules',
      destination: 'session',
      behavior: 'allow',
      rules: allowedPrompts.map((prompt) => ({
        tool_name: prompt.tool,
        rule_content: `${PROMPT_PREFIX} ${prompt.prompt.trim()}`,
      })),
    },
  ];
}

function extractCodexQuestions(input: unknown): CodexQuestion[] {
  const record = asRecord(input);
  const rawQuestions = record?.questions;
  if (!Array.isArray(rawQuestions)) return [];

  const questions: CodexQuestion[] = [];
  for (const item of rawQuestions) {
    const question = asRecord(item);
    const id = typeof question?.id === 'string' ? question.id : '';
    if (!id) continue;

    const options: NonNullable<CodexQuestion['options']> = [];
    if (Array.isArray(question?.options)) {
      for (const option of question.options) {
        const optionRecord = asRecord(option);
        const label = typeof optionRecord?.label === 'string' ? optionRecord.label : '';
        if (!label) continue;
        options.push({
          label,
          description:
            typeof optionRecord?.description === 'string' ? optionRecord.description : undefined,
        });
      }
    }

    questions.push({
      id,
      header: typeof question?.header === 'string' ? question.header : undefined,
      question: typeof question?.question === 'string' ? question.question : undefined,
      options: options.length > 0 ? options : null,
    });
  }
  return questions;
}

function defaultCodexAnswers(questions: CodexQuestion[]): Record<string, { answers: string[] }> {
  return Object.fromEntries(
    questions.map((question) => [
      question.id,
      { answers: question.options?.[0]?.label ? [question.options[0].label] : [] },
    ]),
  );
}

function parseJsonOrText(value: string): unknown {
  const trimmed = value.trim();
  if (!trimmed) return null;
  try {
    return JSON.parse(trimmed);
  } catch {
    return trimmed;
  }
}

export function PermissionModal() {
  const pendingPermission = useAppStore((state) => state.pendingPermission);
  const resolvePermission = useAppStore((state) => state.resolvePermission);
  const privacyMode = useAppStore((state) => state.workspacePrivacyMode);
  const [feedback, setFeedback] = useState('');
  const [codexJsonResponse, setCodexJsonResponse] = useState('');
  const [codexTextResponse, setCodexTextResponse] = useState('');
  const isExitPlanMode = pendingPermission?.tool_name === 'exit_plan_mode';
  const isCodexToolUserInput = pendingPermission?.tool_name === 'tool_user_input';
  const isCodexMcpElicitation = pendingPermission?.tool_name === 'mcp_elicitation';
  const isCodexDynamicTool = pendingPermission?.tool_name === 'dynamic_tool';
  const inputRecord = useMemo(() => asRecord(pendingPermission?.input), [pendingPermission?.input]);
  const rooAsk = stringField(inputRecord, 'ask');
  const isRooFollowup = pendingPermission?.tool_name === 'ask_followup_question' || rooAsk === 'followup';
  const isRooCompletion = pendingPermission?.tool_name === 'attempt_completion' || rooAsk === 'completion_result';
  const isRooMistakeLimit =
    pendingPermission?.tool_name === 'mistake_limit_reached' || rooAsk === 'mistake_limit_reached';
  const isRooTextInteraction = isRooFollowup || isRooCompletion || isRooMistakeLimit;
  const rooQuestionRecord = useMemo(() => asRecord(inputRecord?.question), [inputRecord]);
  const allowedPrompts = useMemo(
    () => extractAllowedPrompts(pendingPermission?.input),
    [pendingPermission?.input],
  );
  const codexQuestions = useMemo(
    () => extractCodexQuestions(pendingPermission?.input),
    [pendingPermission?.input],
  );
  const planText = stringField(inputRecord, 'plan');
  const planFilePath = stringField(inputRecord, 'plan_file_path', 'planFilePath');
  const displayedInput = useMemo(
    () => redactSensitivePathsForDisplay(pendingPermission?.input, privacyMode),
    [pendingPermission?.input, privacyMode],
  );

  useEffect(() => {
    setFeedback('');
    setCodexTextResponse('');
    if (pendingPermission?.tool_name === 'tool_user_input') {
      setCodexJsonResponse(
        JSON.stringify({ answers: defaultCodexAnswers(extractCodexQuestions(pendingPermission.input)) }, null, 2),
      );
    } else if (pendingPermission?.tool_name === 'mcp_elicitation') {
      setCodexJsonResponse(JSON.stringify({ action: 'accept', content: {}, _meta: null }, null, 2));
    } else {
      setCodexJsonResponse('');
    }
  }, [pendingPermission?.request_id]);

  if (!pendingPermission) return null;

  const trimmedFeedback = feedback.trim();
  const rooQuestionText = stringField(rooQuestionRecord, 'question') ?? stringField(inputRecord, 'question');
  const rooCompletionText = stringField(inputRecord, 'result');
  const rooResponseLabel = isRooFollowup
    ? 'Roo 回复'
    : isRooCompletion
    ? 'Roo 完成反馈'
    : 'Roo 继续反馈';

  function denyPermission() {
    if (isCodexMcpElicitation) {
      void resolvePermission({
        allowed: false,
        codex_response: { action: 'decline', content: null, _meta: null },
      });
      return;
    }
    if (isExitPlanMode || isRooTextInteraction) {
      void resolvePermission({
        allowed: false,
        message: trimmedFeedback || null,
        feedback: trimmedFeedback || null,
      });
      return;
    }
    void resolvePermission({ allowed: false });
  }

  function allowPermission() {
    if (isCodexToolUserInput || isCodexMcpElicitation) {
      void resolvePermission({
        allowed: true,
        codex_response: parseJsonOrText(codexJsonResponse),
      });
      return;
    }
    if (isCodexDynamicTool) {
      void resolvePermission({
        allowed: true,
        codex_response: {
          contentItems: [
            {
              type: 'inputText',
              text: codexTextResponse.trim() || 'Approved by user.',
            },
          ],
          success: true,
        },
      });
      return;
    }
    if (isRooTextInteraction) {
      void resolvePermission({
        allowed: true,
        message: trimmedFeedback || null,
        feedback: trimmedFeedback || null,
      });
      return;
    }
    void resolvePermission(
      isExitPlanMode
        ? {
            allowed: true,
            feedback: trimmedFeedback || null,
            permission_updates: buildExitPlanPermissionUpdates(allowedPrompts),
          }
        : { allowed: true },
    );
  }

  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-slate-950/35 p-4 backdrop-blur-[2px]">
      <div className="w-full max-w-2xl rounded-[28px] border border-[#e1d8ca] bg-white shadow-[0_28px_80px_rgba(15,23,42,0.22)]">
        <div className="border-b border-[#efe8dd] px-6 py-5">
          <div className="flex items-center gap-3">
            <div className="flex h-11 w-11 items-center justify-center rounded-2xl bg-[#fff3f2] text-[#b23a2f]">
              <ShieldAlert size={20} />
            </div>
            <div>
              <div className="text-lg font-semibold text-slate-800">权限确认</div>
              <div className="mt-1 text-sm text-slate-500">
                GUI 已收到一个需要人工确认的工具调用。
              </div>
            </div>
          </div>
        </div>

        <div className="space-y-4 px-6 py-5">
          <div>
            <div className="text-sm font-medium text-slate-700">工具</div>
            <div className="mt-1 text-sm text-slate-600">{pendingPermission.tool_name}</div>
          </div>
          <div>
            <div className="text-sm font-medium text-slate-700">说明</div>
            <div className="mt-1 whitespace-pre-wrap text-sm leading-6 text-slate-600">
              {pendingPermission.description}
            </div>
          </div>
          {isExitPlanMode && planText && (
            <div>
              <div className="text-sm font-medium text-slate-700">计划内容</div>
              <pre className="mt-1 max-h-64 overflow-auto rounded-2xl bg-[#f7f5ef] p-4 text-xs leading-6 text-slate-700">
                {planText}
              </pre>
              {planFilePath && (
                <div className="mt-2 break-all text-xs text-slate-500">
                  {formatSensitivePath(planFilePath, privacyMode)}
                </div>
              )}
            </div>
          )}
          {isExitPlanMode && allowedPrompts.length > 0 && (
            <div>
              <div className="text-sm font-medium text-slate-700">请求的语义权限</div>
              <div className="mt-2 space-y-2 rounded-2xl bg-[#f7f5ef] p-4 text-sm text-slate-700">
                {allowedPrompts.map((prompt, index) => (
                  <div key={`${prompt.tool}-${prompt.prompt}-${index}`}>
                    {prompt.tool}({PROMPT_PREFIX} {prompt.prompt})
                  </div>
                ))}
              </div>
            </div>
          )}
          {isCodexToolUserInput && (
            <div className="space-y-3 rounded-2xl border border-[#e3dbcf] bg-[#fbfaf7] p-4">
              <div className="text-sm font-medium text-slate-700">Codex 用户输入请求</div>
              {codexQuestions.map((question) => (
                <div key={question.id} className="rounded-xl bg-white p-3 text-sm text-slate-700">
                  <div className="font-medium">{question.header || question.id}</div>
                  {question.question && <div className="mt-1 text-slate-600">{question.question}</div>}
                  {question.options && question.options.length > 0 && (
                    <div className="mt-2 space-y-1 text-xs text-slate-500">
                      {question.options.map((option) => (
                        <div key={option.label}>
                          {option.label}
                          {option.description ? ` - ${option.description}` : ''}
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              ))}
              <label htmlFor="codex-user-input-response" className="text-sm font-medium text-slate-700">
                官方 ToolRequestUserInputResponse JSON
              </label>
              <textarea
                id="codex-user-input-response"
                value={codexJsonResponse}
                onChange={(event) => setCodexJsonResponse(event.target.value)}
                className="min-h-40 w-full rounded-2xl border border-[#e3dbcf] bg-white px-4 py-3 font-mono text-xs leading-5 text-slate-700 outline-none transition focus:border-slate-400"
              />
            </div>
          )}
          {isCodexMcpElicitation && (
            <div className="space-y-3 rounded-2xl border border-[#e3dbcf] bg-[#fbfaf7] p-4">
              <div className="text-sm font-medium text-slate-700">Codex MCP elicitation</div>
              <div className="text-sm text-slate-600">
                填写官方 `McpServerElicitationRequestResponse`。拒绝按钮会返回 decline。
              </div>
              <textarea
                aria-label="MCP elicitation response"
                value={codexJsonResponse}
                onChange={(event) => setCodexJsonResponse(event.target.value)}
                className="min-h-40 w-full rounded-2xl border border-[#e3dbcf] bg-white px-4 py-3 font-mono text-xs leading-5 text-slate-700 outline-none transition focus:border-slate-400"
              />
            </div>
          )}
          {isCodexDynamicTool && (
            <div className="space-y-3 rounded-2xl border border-[#e3dbcf] bg-[#fbfaf7] p-4">
              <label htmlFor="codex-dynamic-tool-output" className="text-sm font-medium text-slate-700">
                Codex dynamic tool 输出
              </label>
              <textarea
                id="codex-dynamic-tool-output"
                value={codexTextResponse}
                onChange={(event) => setCodexTextResponse(event.target.value)}
                placeholder="返回给官方 DynamicToolCallResponse 的文本。"
                className="min-h-28 w-full rounded-2xl border border-[#e3dbcf] bg-white px-4 py-3 text-sm leading-6 text-slate-700 outline-none transition focus:border-slate-400"
              />
            </div>
          )}
          {isRooFollowup && rooQuestionText && (
            <div className="rounded-2xl border border-[#e3dbcf] bg-[#fbfaf7] p-4 text-sm leading-6 text-slate-700">
              {rooQuestionText}
            </div>
          )}
          {isRooCompletion && rooCompletionText && (
            <pre className="max-h-64 overflow-auto rounded-2xl bg-[#f7f5ef] p-4 text-sm leading-6 text-slate-700">
              {rooCompletionText}
            </pre>
          )}
          {isRooTextInteraction && (
            <div>
              <label htmlFor="roo-permission-feedback" className="text-sm font-medium text-slate-700">
                {rooResponseLabel}
              </label>
              <textarea
                id="roo-permission-feedback"
                value={feedback}
                onChange={(event) => setFeedback(event.target.value)}
                placeholder={
                  isRooCompletion
                    ? '留空表示接受结果；填写内容会作为反馈继续执行。'
                    : '填写要返回给 Roo 的补充信息。'
                }
                className="mt-2 min-h-28 w-full rounded-2xl border border-[#e3dbcf] bg-white px-4 py-3 text-sm leading-6 text-slate-700 outline-none transition focus:border-slate-400"
              />
            </div>
          )}
          {pendingPermission.blocked_path && (
            <div>
              <div className="text-sm font-medium text-slate-700">目标路径</div>
              <div className="mt-1 break-all rounded-2xl bg-[#f7f5ef] px-3 py-2 text-sm text-slate-600">
                {formatSensitivePath(pendingPermission.blocked_path, privacyMode)}
              </div>
            </div>
          )}
          {pendingPermission.permission_suggestions.length > 0 && (
            <div>
              <div className="text-sm font-medium text-slate-700">权限建议</div>
              <div className="mt-1 space-y-2">
                {pendingPermission.permission_suggestions.map((suggestion, index) => (
                  <pre
                    key={index}
                    className="max-h-40 overflow-auto rounded-2xl bg-[#f7f5ef] p-4 text-xs leading-6 text-slate-700"
                  >
                    {formatInput(redactSensitivePathsForDisplay(suggestion, privacyMode))}
                  </pre>
                ))}
              </div>
            </div>
          )}
          <div>
            <div className="text-sm font-medium text-slate-700">输入参数</div>
            <pre className="mt-1 max-h-64 overflow-auto rounded-2xl bg-[#f7f5ef] p-4 text-xs leading-6 text-slate-700">
              {formatInput(displayedInput)}
            </pre>
          </div>
          {isExitPlanMode && (
            <div>
              <label htmlFor="permission-feedback" className="text-sm font-medium text-slate-700">
                审批反馈
              </label>
              <textarea
                id="permission-feedback"
                value={feedback}
                onChange={(event) => setFeedback(event.target.value)}
                placeholder="可选：补充执行要求或拒绝原因。"
                className="mt-2 min-h-28 w-full rounded-2xl border border-[#e3dbcf] bg-white px-4 py-3 text-sm leading-6 text-slate-700 outline-none transition focus:border-slate-400"
              />
            </div>
          )}
        </div>

        <div className="flex justify-end gap-3 border-t border-[#efe8dd] bg-[#fbfaf7] px-6 py-4">
          <button
            onClick={denyPermission}
            className="rounded-2xl border border-[#e3dbcf] px-4 py-2 text-sm font-medium text-slate-600 transition-colors hover:bg-white"
          >
            拒绝
          </button>
          <button
            onClick={allowPermission}
            className="rounded-2xl bg-[#17181a] px-5 py-2 text-sm font-medium text-white transition-colors hover:bg-[#2b2d31]"
          >
            允许执行
          </button>
        </div>
      </div>
    </div>
  );
}
