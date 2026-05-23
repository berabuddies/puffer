/**
 * Home page task feed — strings match the design (PK-0) verbatim.
 *
 * Both groups carry their own count so the section eyebrows can render
 * "Work 3" / "Life 1" without recomputing from `tasks.length`.
 */

import type { TaskGroup } from "./types";

export const workTasks: TaskGroup = {
  label: "Work",
  count: 3,
  tasks: [
    {
      id: "work-onboard-austin",
      icon: "calendar",
      title: "Onboard Austin and Tech Discussion",
      meta: "Hongbo Wen 发的会议邀请，时间是5月20日 20:45-21:15 (GMT-7)",
      primaryAction: { label: "Open Lark", tone: "cream" },
      secondaryAction: { label: "Ignore", tone: "neutral" }
    },
    {
      id: "work-worldclaw-cancelled",
      icon: "calendar",
      title: "WorldClaw每日例会 — 已取消",
      meta: "Kristie Guo 今天下午3:03发来通知，你每天12:30-13:00的站会被取消了。",
      primaryAction: { label: "Open Lark", tone: "cream" },
      secondaryAction: { label: "Ignore", tone: "neutral" }
    },
    {
      id: "work-figma-reviews",
      icon: "message-circle",
      title: "Figma — WorldAgent 8条新评论",
      meta: "Rose希望品牌特点能更突出，这些反馈需要尽快看看。",
      primaryAction: { label: "Open Figma", tone: "cream" },
      secondaryAction: { label: "Ignore", tone: "neutral" }
    }
  ]
};

export const lifeTasks: TaskGroup = {
  label: "Life",
  count: 2,
  tasks: [
    {
      id: "life-kim-birthday",
      icon: "cake",
      title: "Kim下周五生日",
      meta: "让我为你们预约一家涩谷的晚饭？别忘了她海鲜过敏",
      primaryAction: { label: "Book Restaurant", tone: "cream", navigateTo: "/agent/restaurant" },
      secondaryAction: { label: "Ignore", tone: "neutral" }
    },
    {
      id: "life-dog-food",
      icon: "shopping-bag",
      title: "今天是你买狗粮的日子",
      meta: "让我为你在亚马逊上下单",
      primaryAction: { label: "Buy", tone: "cream" },
      secondaryAction: { label: "Ignore", tone: "neutral" }
    }
  ]
};

export const allTaskGroups: TaskGroup[] = [workTasks, lifeTasks];
