import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ActivityItem } from "../bindings/ActivityItem";
import type { AnalysisError } from "../bindings/AnalysisError";
import type { ChatEvent } from "../bindings/ChatEvent";
import type { ChatTranscript } from "../bindings/ChatTranscript";
import type { AuditEntry } from "../bindings/AuditEntry";
import type { ChangeKind } from "../bindings/ChangeKind";
import type { RepoPriority } from "../bindings/RepoPriority";
import type { LogEntry } from "../bindings/LogEntry";
import type { PrCommit } from "../bindings/PrCommit";
import type { PrConversation } from "../bindings/PrConversation";
import type { PrReviews } from "../bindings/PrReviews";
import type { PrPriority } from "../bindings/PrPriority";
import type { AnalysisLevel } from "../bindings/AnalysisLevel";
import type { AnalysisProgress } from "../bindings/AnalysisProgress";
import type { AnalysisResult } from "../bindings/AnalysisResult";
import type { PollStatus } from "../bindings/PollStatus";
import type { PrChangedEvent } from "../bindings/PrChangedEvent";
import type { ReviewMark } from "../bindings/ReviewMark";
import type { Settings } from "../bindings/Settings";
import type { TrackedPr } from "../bindings/TrackedPr";

export const events = {
  prsSnapshot: "prs:snapshot",
  prChanged: "pr:changed",
  pollStatus: "poll:status",
  focusPr: "focus:pr",
  analysisProgress: "analysis:progress",
  analysisComplete: "analysis:complete",
  analysisError: "analysis:error",
  chatEvent: "chat:event",
} as const;

export const ipc = {
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
  setGithubPat: (token: string) => invoke<void>("set_github_pat", { token }),
  githubPatPresent: () => invoke<boolean>("github_pat_present"),
  clearGithubPat: () => invoke<void>("clear_github_pat"),
  listPrs: () => invoke<TrackedPr[]>("list_prs"),
  markPrRead: (id: string) => invoke<void>("mark_pr_read", { id }),
  markPrReadKinds: (id: string, kinds: ChangeKind[]) =>
    invoke<void>("mark_pr_read_kinds", { id, kinds }),
  setPrMuted: (id: string, muted: boolean) => invoke<void>("set_pr_muted", { id, muted }),
  untrackPr: (id: string) => invoke<void>("untrack_pr", { id }),
  trackPrUrl: (url: string) => invoke<TrackedPr>("track_pr_url", { url }),
  pollNow: () => invoke<void>("poll_now"),
  getAnalysis: (prId: string, level: AnalysisLevel, focus?: string) =>
    invoke<AnalysisResult | null>("get_analysis", { prId, level, focus: focus ?? null }),
  runAnalysis: (prId: string, level: AnalysisLevel, focus?: string, force?: boolean) =>
    invoke<void>("run_analysis", { prId, level, focus: focus ?? null, force: force ?? false }),
  getPrDiff: (prId: string) => invoke<string>("get_pr_diff", { prId }),
  chatHistory: (prId: string) => invoke<ChatTranscript>("chat_history", { prId }),
  chatSend: (prId: string, text: string) => invoke<void>("chat_send", { prId, text }),
  chatConfirm: (prId: string, approve: boolean) =>
    invoke<void>("chat_confirm", { prId, approve }),
  chatClear: (prId: string) => invoke<void>("chat_clear", { prId }),
  ensureReviewMark: (prId: string) => invoke<ReviewMark>("ensure_review_mark", { prId }),
  setReviewMark: (prId: string) => invoke<ReviewMark>("set_review_mark", { prId }),
  getDiffSince: (prId: string) => invoke<string>("get_diff_since", { prId }),
  setPrPriority: (id: string, priority: PrPriority) =>
    invoke<void>("set_pr_priority", { id, priority }),
  setRepoPriority: (repo: string, priority: RepoPriority) =>
    invoke<void>("set_repo_priority", { repo, priority }),
  getAuditLog: () => invoke<AuditEntry[]>("get_audit_log"),
  undoAudit: (id: number) => invoke<void>("undo_audit", { id }),
  getActivity: () => invoke<ActivityItem[]>("get_activity"),
  markActivityRead: (ids: number[], read: boolean) =>
    invoke<void>("mark_activity_read", { ids, read }),
  setActivityFlag: (id: number, flag: string) => invoke<void>("set_activity_flag", { id, flag }),
  getPrComments: (prId: string) => invoke<PrConversation>("get_pr_comments", { prId }),
  getPrCommits: (prId: string) => invoke<PrCommit[]>("get_pr_commits", { prId }),
  refreshPr: (prId: string) => invoke<TrackedPr>("refresh_pr", { prId }),
  getPrReviews: (prId: string) => invoke<PrReviews>("get_pr_reviews", { prId }),
  mergePr: (prId: string, method: "squash" | "merge" | "rebase") =>
    invoke<void>("merge_pr", { prId, method }),
  submitReview: (prId: string, event: "approve" | "request-changes" | "comment", body: string) =>
    invoke<void>("submit_review", { prId, event, body }),
  resolveThread: (threadId: string, resolve: boolean) =>
    invoke<void>("resolve_thread", { threadId, resolve }),
  closePr: (prId: string) => invoke<void>("close_pr", { prId }),
  reopenPr: (prId: string) => invoke<void>("reopen_pr", { prId }),
  addPrComment: (prId: string, body: string) =>
    invoke<void>("add_pr_comment", { prId, body }),
  replyToThread: (threadId: string, body: string) =>
    invoke<void>("reply_to_thread", { threadId, body }),
  addDiffComment: (prId: string, path: string, line: number, body: string) =>
    invoke<void>("add_diff_comment", { prId, path, line, body }),
  toggleReaction: (subjectId: string, content: string, remove: boolean) =>
    invoke<void>("toggle_reaction", { subjectId, content, remove }),
  takePendingFocus: () =>
    invoke<{ prId: string; commentId: string | null } | null>("take_pending_focus"),
  getViewedFiles: (prId: string) =>
    invoke<{ path: string; digest: string }[]>("get_viewed_files", { prId }),
  setFileViewed: (prId: string, path: string, digest: string, viewed: boolean) =>
    invoke<void>("set_file_viewed", { prId, path, digest, viewed }),
  getFileAtHead: (prId: string, path: string) =>
    invoke<string>("get_file_at_head", { prId, path }),
  awsSsoLogin: (profile: string) => invoke<void>("aws_sso_login", { profile }),
  checkAws: (profile: string, region: string) =>
    invoke<string>("check_aws", { profile, region }),
  getDevLogs: () => invoke<LogEntry[]>("get_dev_logs"),
  clearDevLogs: () => invoke<void>("clear_dev_logs"),
  getDefaultSystemPrompt: () => invoke<string>("get_default_system_prompt"),
  getAppInternals: () =>
    invoke<{ dataDir: string; dbPath: string; version: string }>("get_app_internals"),
  showMainWindow: (prId?: string) => invoke<void>("show_main_window", { prId: prId ?? null }),
  toggleCallout: () => invoke<void>("toggle_callout"),
};

export function onPrsSnapshot(cb: (prs: TrackedPr[]) => void): Promise<UnlistenFn> {
  return listen<TrackedPr[]>(events.prsSnapshot, (e) => cb(e.payload));
}

export function onPrChanged(cb: (e: PrChangedEvent) => void): Promise<UnlistenFn> {
  return listen<PrChangedEvent>(events.prChanged, (e) => cb(e.payload));
}

export function onPollStatus(cb: (s: PollStatus) => void): Promise<UnlistenFn> {
  return listen<PollStatus>(events.pollStatus, (e) => cb(e.payload));
}

export function onFocusPr(cb: (id: string) => void): Promise<UnlistenFn> {
  return listen<string>(events.focusPr, (e) => cb(e.payload));
}

export function onAnalysisProgress(cb: (p: AnalysisProgress) => void): Promise<UnlistenFn> {
  return listen<AnalysisProgress>(events.analysisProgress, (e) => cb(e.payload));
}

export function onAnalysisComplete(cb: (r: AnalysisResult) => void): Promise<UnlistenFn> {
  return listen<AnalysisResult>(events.analysisComplete, (e) => cb(e.payload));
}

export function onAnalysisError(cb: (e: AnalysisError) => void): Promise<UnlistenFn> {
  return listen<AnalysisError>(events.analysisError, (e) => cb(e.payload));
}

export function onDevLog(cb: (entry: LogEntry) => void): Promise<UnlistenFn> {
  return listen<LogEntry>("dev:log", (e) => cb(e.payload));
}

export function onChatEvent(cb: (e: ChatEvent) => void): Promise<UnlistenFn> {
  return listen<ChatEvent>(events.chatEvent, (e) => cb(e.payload));
}
