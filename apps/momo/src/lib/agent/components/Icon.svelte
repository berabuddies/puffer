<script lang="ts" module>
  export type IconName =
    | "file"
    | "folder"
    | "folderOpen"
    | "edit"
    | "terminal"
    | "play"
    | "pause"
    | "pause2"
    | "plus"
    | "sparkles"
    | "arrow"
    | "arrowUp"
    | "chevR"
    | "chevD"
    | "chevL"
    | "git"
    | "branch"
    | "globe"
    | "check"
    | "x"
    | "settings"
    | "test"
    | "search"
    | "plug"
    | "bolt"
    | "bug"
    | "repo"
    | "clock"
    | "cpu"
    | "moreH"
    | "eye"
    | "eyeOff"
    | "panel"
    | "panelOpen"
    | "panelClose"
    | "layers"
    | "refresh"
    | "flame"
    | "rocket"
    | "key"
    | "lock"
    | "shield"
    | "server"
    | "link"
    | "logs"
    | "listTodo"
    | "external"
    | "copy"
    | "coin"
    | "token"
    | "wrench"
    | "pin"
    | "bot";
</script>

<script lang="ts">
  import ArrowRightIcon from "lucide-svelte/icons/arrow-right";
  import ArrowUpIcon from "lucide-svelte/icons/arrow-up";
  import BadgeCentIcon from "lucide-svelte/icons/badge-cent";
  import BotIcon from "lucide-svelte/icons/bot";
  import BugIcon from "lucide-svelte/icons/bug";
  import CheckIcon from "lucide-svelte/icons/check";
  import ChevronDownIcon from "lucide-svelte/icons/chevron-down";
  import ChevronLeftIcon from "lucide-svelte/icons/chevron-left";
  import ChevronRightIcon from "lucide-svelte/icons/chevron-right";
  import CircleEllipsisIcon from "lucide-svelte/icons/circle-ellipsis";
  import Clock3Icon from "lucide-svelte/icons/clock-3";
  import CoinsIcon from "lucide-svelte/icons/coins";
  import CopyIcon from "lucide-svelte/icons/copy";
  import CpuIcon from "lucide-svelte/icons/cpu";
  import EyeIcon from "lucide-svelte/icons/eye";
  import EyeOffIcon from "lucide-svelte/icons/eye-off";
  import ExternalLinkIcon from "lucide-svelte/icons/external-link";
  import FileIcon from "lucide-svelte/icons/file";
  import FlameIcon from "lucide-svelte/icons/flame";
  import FlaskConicalIcon from "lucide-svelte/icons/flask-conical";
  import FolderGit2Icon from "lucide-svelte/icons/folder-git-2";
  import FolderIcon from "lucide-svelte/icons/folder";
  import FolderOpenIcon from "lucide-svelte/icons/folder-open";
  import GitBranchIcon from "lucide-svelte/icons/git-branch";
  import GitGraphIcon from "lucide-svelte/icons/git-graph";
  import GlobeIcon from "lucide-svelte/icons/globe";
  import KeyRoundIcon from "lucide-svelte/icons/key-round";
  import LayersIcon from "lucide-svelte/icons/layers";
  import LayoutPanelLeftIcon from "lucide-svelte/icons/layout-panel-left";
  import LinkIcon from "lucide-svelte/icons/link";
  import LockIcon from "lucide-svelte/icons/lock";
  import LogsIcon from "lucide-svelte/icons/logs";
  import ListTodoIcon from "lucide-svelte/icons/list-todo";
  import MoreHorizontalIcon from "lucide-svelte/icons/more-horizontal";
  import PanelLeftCloseIcon from "lucide-svelte/icons/panel-left-close";
  import PanelLeftOpenIcon from "lucide-svelte/icons/panel-left-open";
  import PauseIcon from "lucide-svelte/icons/pause";
  import PencilIcon from "lucide-svelte/icons/pencil";
  import PinIcon from "lucide-svelte/icons/pin";
  import PlayIcon from "lucide-svelte/icons/play";
  import PlugIcon from "lucide-svelte/icons/plug";
  import PlusIcon from "lucide-svelte/icons/plus";
  import RefreshCwIcon from "lucide-svelte/icons/refresh-cw";
  import RocketIcon from "lucide-svelte/icons/rocket";
  import SearchIcon from "lucide-svelte/icons/search";
  import ServerIcon from "lucide-svelte/icons/server";
  import SettingsIcon from "lucide-svelte/icons/settings";
  import ShieldIcon from "lucide-svelte/icons/shield";
  import SparklesIcon from "lucide-svelte/icons/sparkles";
  import SquarePauseIcon from "lucide-svelte/icons/square-pause";
  import TerminalIcon from "lucide-svelte/icons/terminal";
  import WrenchIcon from "lucide-svelte/icons/wrench";
  import XIcon from "lucide-svelte/icons/x";
  import ZapIcon from "lucide-svelte/icons/zap";

  type Props = {
    name: IconName;
    size?: number;
    color?: string;
    strokeWidth?: number;
  };

  const iconMap: Record<IconName, any> = {
    file: FileIcon,
    folder: FolderIcon,
    folderOpen: FolderOpenIcon,
    edit: PencilIcon,
    terminal: TerminalIcon,
    play: PlayIcon,
    pause: PauseIcon,
    pause2: SquarePauseIcon,
    plus: PlusIcon,
    sparkles: SparklesIcon,
    arrow: ArrowRightIcon,
    arrowUp: ArrowUpIcon,
    chevR: ChevronRightIcon,
    chevD: ChevronDownIcon,
    chevL: ChevronLeftIcon,
    git: GitGraphIcon,
    branch: GitBranchIcon,
    globe: GlobeIcon,
    check: CheckIcon,
    x: XIcon,
    settings: SettingsIcon,
    test: FlaskConicalIcon,
    search: SearchIcon,
    plug: PlugIcon,
    bolt: ZapIcon,
    bug: BugIcon,
    repo: FolderGit2Icon,
    clock: Clock3Icon,
    cpu: CpuIcon,
    moreH: MoreHorizontalIcon,
    eye: EyeIcon,
    eyeOff: EyeOffIcon,
    panel: LayoutPanelLeftIcon,
    panelOpen: PanelLeftOpenIcon,
    panelClose: PanelLeftCloseIcon,
    layers: LayersIcon,
    refresh: RefreshCwIcon,
    flame: FlameIcon,
    rocket: RocketIcon,
    key: KeyRoundIcon,
    lock: LockIcon,
    shield: ShieldIcon,
    server: ServerIcon,
    link: LinkIcon,
    logs: LogsIcon,
    listTodo: ListTodoIcon,
    external: ExternalLinkIcon,
    copy: CopyIcon,
    coin: CoinsIcon,
    token: BadgeCentIcon,
    wrench: WrenchIcon,
    pin: PinIcon,
    bot: BotIcon
  };

  let { name, size = 14, color, strokeWidth = 1.8 }: Props = $props();

  let IconComponent = $derived(iconMap[name] ?? CircleEllipsisIcon);
</script>

<IconComponent size={size} color={color} strokeWidth={strokeWidth} />
