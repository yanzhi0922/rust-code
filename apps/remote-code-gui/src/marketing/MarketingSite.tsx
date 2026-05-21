import {
  ArrowRight,
  CheckCircle2,
  Download,
  ExternalLink,
  MonitorOff,
  Server,
  ShieldCheck,
  Smartphone,
  TerminalSquare,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

type Feature = {
  icon: LucideIcon;
  title: string;
  description: string;
};

const domainRoles: Feature[] = [
  {
    icon: TerminalSquare,
    title: '官网展示',
    description: '浏览器打开后只展示产品介绍、部署边界和下载指引，不再进入远程会话控制台。',
  },
  {
    icon: Server,
    title: '控制面中继',
    description: '同一域名仍可承载 control-plane 的健康检查、设备认证、runner 中继和受控下载接口。',
  },
  {
    icon: Smartphone,
    title: '手机 App 远控',
    description: '发送 prompt、处理审批、查看 timeline 和下载产物只保留在原生手机 App 路径里。',
  },
];

const productFeatures: Feature[] = [
  {
    icon: ShieldCheck,
    title: '本机保留执行权',
    description: '代码仓库、provider key、agent loop 和工具调用留在可信桌面或本地 runner 中。',
  },
  {
    icon: CheckCircle2,
    title: '审批优先',
    description: '远程操作以审批、事件流和产物为边界，关键动作需要受信设备确认。',
  },
  {
    icon: MonitorOff,
    title: 'Web 端无控制台',
    description: '普通浏览器访问不会加载远程控制界面，也不会接收配对链接或长期 token。',
  },
];

function FeatureCell({ icon: Icon, title, description }: Feature) {
  return (
    <article className="rounded-md border border-[#d8ded8] bg-white/82 p-5 shadow-sm">
      <div className="mb-4 flex h-10 w-10 items-center justify-center rounded-md bg-[#0f766e] text-white">
        <Icon size={20} aria-hidden="true" />
      </div>
      <h3 className="text-lg font-semibold text-[#111827]">{title}</h3>
      <p className="mt-3 text-sm leading-6 text-[#4b5563]">{description}</p>
    </article>
  );
}

function ProductVisual() {
  return (
    <div className="relative self-center overflow-hidden rounded-md border border-[#1f2937] bg-[#10151f] shadow-2xl">
      <div className="flex h-10 items-center justify-between border-b border-white/10 px-4">
        <div className="flex items-center gap-2">
          <span className="h-2.5 w-2.5 rounded-full bg-[#ef4444]" />
          <span className="h-2.5 w-2.5 rounded-full bg-[#f59e0b]" />
          <span className="h-2.5 w-2.5 rounded-full bg-[#10b981]" />
        </div>
        <span className="text-xs font-medium text-slate-400">mobile trusted route</span>
      </div>
      <div className="grid gap-0 md:grid-cols-[1.15fr_0.85fr]">
        <div className="border-b border-white/10 p-5 md:border-b-0 md:border-r">
          <div className="mb-5 flex items-center gap-3">
            <img src="/brand-mark.svg" alt="" className="h-11 w-11 rounded-md bg-slate-900 p-1.5" draggable={false} />
            <div>
              <div className="text-sm font-semibold text-white">Remote Code</div>
              <div className="text-xs text-slate-400">local runner online</div>
            </div>
          </div>
          <div className="space-y-3">
            <div className="rounded-md border border-white/10 bg-white/[0.04] p-4">
              <div className="text-xs uppercase text-slate-500">approval</div>
              <div className="mt-2 text-sm leading-6 text-slate-100">修改前端入口并确认 Web 端不暴露远程控制。</div>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="rounded-md border border-emerald-400/30 bg-emerald-400/10 p-3 text-sm font-semibold text-emerald-200">
                手机 App
              </div>
              <div className="rounded-md border border-slate-500/30 bg-slate-700/20 p-3 text-sm font-semibold text-slate-300">
                浏览器官网
              </div>
            </div>
          </div>
        </div>
        <div className="bg-[#0d111a] p-5">
          <div className="mb-4 text-xs font-semibold uppercase text-slate-500">boundary</div>
          <div className="space-y-3">
            {['Web 控制台关闭', 'Relay 只做中继', 'Runner 保持本地'].map((item) => (
              <div key={item} className="flex items-center gap-3 rounded-md border border-white/10 bg-white/[0.035] p-3">
                <CheckCircle2 size={16} className="text-[#2dd4bf]" aria-hidden="true" />
                <span className="text-sm text-slate-200">{item}</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

export function MarketingSite() {
  return (
    <main className="h-[100dvh] overflow-y-auto bg-[#f6f3ec] text-[#111827] antialiased">
      <header className="border-b border-[#d8ded8] bg-[#f6f3ec]/95">
        <div className="mx-auto flex max-w-7xl items-center justify-between px-5 py-4 md:px-8">
          <a href="/" className="flex items-center gap-3 text-[#111827] no-underline" aria-label="Remote Code 首页">
            <img src="/brand-mark.svg" alt="" className="h-9 w-9" draggable={false} />
            <span className="text-base font-semibold">Remote Code</span>
          </a>
          <nav className="hidden items-center gap-6 text-sm font-medium md:flex" aria-label="主要导航">
            <a className="text-[#4b5563] no-underline hover:text-[#0f766e]" href="#domain">域名用途</a>
            <a className="text-[#4b5563] no-underline hover:text-[#0f766e]" href="#boundary">控制边界</a>
            <a className="text-[#4b5563] no-underline hover:text-[#0f766e]" href="#download">移动 App</a>
          </nav>
        </div>
      </header>

      <section className="mx-auto grid max-w-7xl gap-10 px-5 py-12 md:grid-cols-[0.9fr_1.1fr] md:px-8 md:py-16">
        <div className="flex flex-col justify-center">
          <div className="mb-5 inline-flex w-fit items-center gap-2 rounded-md border border-[#c9d4ce] bg-white/80 px-3 py-2 text-sm font-semibold text-[#0f766e]">
            <ShieldCheck size={16} aria-hidden="true" />
            手机 App 专用远控
          </div>
          <h1 className="max-w-3xl text-4xl font-bold leading-tight text-[#0f172a] md:text-6xl">
            把 AI 编程环境留在本机，远程控制交给手机 App。
          </h1>
          <p className="mt-6 max-w-2xl text-base leading-7 text-[#4b5563] md:text-lg">
            remote-code.yz520gzy.top 现在作为 Remote Code 的产品官网和云端控制面入口。
            普通浏览器只用于了解产品，不提供发送 prompt、审批、timeline 或 artifact 控制台。
          </p>
          <div className="mt-8 flex flex-col gap-3 sm:flex-row">
            <a
              href="#download"
              className="inline-flex items-center justify-center gap-2 rounded-md bg-[#0f766e] px-5 py-3 text-sm font-semibold text-white no-underline shadow-sm transition hover:bg-[#115e59] active:scale-[0.98]"
            >
              <Smartphone size={18} aria-hidden="true" />
              查看移动 App 入口
            </a>
            <a
              href="https://github.com/yanzhi0922/remote-code-rust"
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center justify-center gap-2 rounded-md border border-[#c9d4ce] bg-white px-5 py-3 text-sm font-semibold text-[#111827] no-underline transition hover:border-[#0f766e] active:scale-[0.98]"
            >
              <ExternalLink size={18} aria-hidden="true" />
              GitHub
            </a>
          </div>
        </div>
        <ProductVisual />
      </section>

      <section id="domain" className="border-y border-[#d8ded8] bg-[#eef2ea] px-5 py-12 md:px-8">
        <div className="mx-auto max-w-7xl">
          <div className="mb-8 max-w-2xl">
            <p className="text-sm font-semibold text-[#0f766e]">remote-code.yz520gzy.top</p>
            <h2 className="mt-2 text-3xl font-bold text-[#111827]">这个域名现在承担三件事</h2>
          </div>
          <div className="grid gap-4 md:grid-cols-3">
            {domainRoles.map((feature) => (
              <FeatureCell key={feature.title} {...feature} />
            ))}
          </div>
        </div>
      </section>

      <section id="boundary" className="mx-auto max-w-7xl px-5 py-12 md:px-8 md:py-16">
        <div className="grid gap-8 md:grid-cols-[0.8fr_1.2fr]">
          <div>
            <p className="text-sm font-semibold text-[#0f766e]">产品边界</p>
            <h2 className="mt-2 text-3xl font-bold text-[#111827]">远程能力保留在受信客户端</h2>
            <p className="mt-4 text-base leading-7 text-[#4b5563]">
              Web 官网不做远程控制入口。控制面仍然为手机 App、桌面 runner 和受信设备提供 relay 能力，
              但浏览器访问不会进入会话操作界面。
            </p>
          </div>
          <div className="grid gap-4 md:grid-cols-3">
            {productFeatures.map((feature) => (
              <FeatureCell key={feature.title} {...feature} />
            ))}
          </div>
        </div>
      </section>

      <section id="download" className="bg-[#111827] px-5 py-12 text-white md:px-8">
        <div className="mx-auto grid max-w-7xl gap-8 md:grid-cols-[1fr_auto] md:items-center">
          <div>
            <p className="text-sm font-semibold text-[#2dd4bf]">移动 App</p>
            <h2 className="mt-2 text-3xl font-bold">远程控制只在手机 App 内启用</h2>
            <p className="mt-4 max-w-2xl text-base leading-7 text-slate-300">
              安装包和内测版本通过受信下载页或 GitHub Release 分发。公网 Web 首页保持官网模式，
              不接受远程会话登录、配对或操作请求。
            </p>
          </div>
          <div className="flex flex-col gap-3 sm:flex-row">
            <a
              href="https://github.com/yanzhi0922/remote-code-rust/releases"
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center justify-center gap-2 rounded-md bg-white px-5 py-3 text-sm font-semibold text-[#111827] no-underline transition hover:bg-slate-100 active:scale-[0.98]"
            >
              <Download size={18} aria-hidden="true" />
              查看发布版本
            </a>
            <a
              href="#domain"
              className="inline-flex items-center justify-center gap-2 rounded-md border border-white/20 px-5 py-3 text-sm font-semibold text-white no-underline transition hover:border-[#2dd4bf] active:scale-[0.98]"
            >
              域名用途
              <ArrowRight size={18} aria-hidden="true" />
            </a>
          </div>
        </div>
      </section>
    </main>
  );
}
