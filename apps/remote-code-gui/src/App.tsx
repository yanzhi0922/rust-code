import { lazy, Suspense, useState, useEffect, useCallback } from 'react';
import { LockKeyhole, TriangleAlert } from 'lucide-react';
import { Layout } from './components/layout/Layout';
import { PermissionModal } from './components/layout/PermissionModal';
import { ChatArea } from './components/chat/ChatArea';
import { ChatInput } from './components/chat/ChatInput';
import { WorkspaceOverview } from './components/layout/WorkspaceOverview';
import { ThemeProvider } from './components/design/ThemeProvider';
import { AppErrorBoundary } from './components/layout/AppErrorBoundary';
import { hasTauriRuntime, shouldUseRemoteMode } from './lib/runtime';
import { isMobileSync, isTouchDevice } from './lib/mobile';
import { MarketingSite } from './marketing/MarketingSite';
import {
  performBiometricCheck,
  initNetworkMonitoring,
  getNetworkStatus,
  onNetworkChange,
  describeConnectionType,
  hapticSuccess,
  hapticError,
  hapticWarning,
} from './lib/mobile';
import { useAppStore } from './stores/useAppStore';

const MobileRemoteApp = lazy(() => import('./remote/MobileRemoteApp'));

type MobileInitPhase = 'loading' | 'biometric' | 'ready' | 'error';

function MobileInitScreen() {
  return (
    <div className="flex h-screen w-screen items-center justify-center bg-rc-bg-base">
      <div className="flex flex-col items-center gap-4">
        <img src="/pwa-icon-192.png" alt="" className="h-14 w-14 rounded-2xl shadow-lg" draggable={false} />
        <div className="flex items-center gap-3 text-rc-text-secondary">
          <div role="status" className="h-5 w-5 rounded-full border-2 border-rc-border-primary border-t-rc-text-primary animate-spin" />
          <span className="text-sm font-medium">正在初始化...</span>
        </div>
      </div>
    </div>
  );
}

function MobileBiometricScreen() {
  return (
    <div className="flex h-screen w-screen items-center justify-center bg-rc-bg-base">
      <div className="flex flex-col items-center gap-4">
        <div className="h-14 w-14 rounded-2xl bg-rc-bg-user-bubble flex items-center justify-center shadow-lg">
          <LockKeyhole size={24} className="text-rc-text-inverse" />
        </div>
        <p className="text-sm text-rc-text-secondary font-medium">请验证身份</p>
      </div>
    </div>
  );
}

function MobileErrorScreen({ error, onRetry }: { error: string; onRetry: () => void }) {
  return (
    <div className="flex h-screen w-screen items-center justify-center bg-rc-bg-base px-6">
      <div className="max-w-sm text-center space-y-4">
        <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-lg bg-rc-accent-error-bg text-rc-accent-error">
          <TriangleAlert size={24} />
        </div>
        <h1 className="text-lg font-bold text-rc-text-primary">初始化失败</h1>
        <p role="alert" className="text-sm text-rc-text-secondary break-all">{error}</p>
        <button
          onClick={onRetry}
          className="px-4 py-2 bg-rc-bg-user-bubble text-rc-text-inverse rounded-lg text-sm font-medium hover:opacity-90 transition-colors"
        >
          重试
        </button>
      </div>
    </div>
  );
}

function MobileNetworkBanner({ online, connectionType }: { online: boolean; connectionType: string }) {
  if (online) return null;
  return (
    <div role="alert" className="fixed top-0 left-0 right-0 z-50 bg-rc-accent-warning text-rc-text-inverse text-center py-1.5 text-xs font-medium shadow-md">
      网络已断开 — {describeConnectionType(connectionType)}
    </div>
  );
}

function MobileGate({ children }: { children: React.ReactNode }) {
  const [phase, setPhase] = useState<MobileInitPhase>('loading');
  const [error, setError] = useState<string | null>(null);
  const [networkOnline, setNetworkOnline] = useState(true);
  const [connectionType, setConnectionType] = useState('unknown');

  const initialize = useCallback(async () => {
    try {
      initNetworkMonitoring();
      const netStatus = getNetworkStatus();
      setNetworkOnline(netStatus.connected);
      setConnectionType(netStatus.connectionType);

      setPhase('biometric');
      const bioOk = await performBiometricCheck();
      if (!bioOk) {
        hapticError();
        setError('身份验证失败');
        setPhase('error');
        return;
      }

      hapticSuccess();
      setPhase('ready');
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      setPhase('error');
    }
  }, []);

  useEffect(() => {
    const unsubscribe = onNetworkChange((connected, type) => {
      setNetworkOnline(connected);
      setConnectionType(type);
      if (!connected) hapticWarning();
    });
    return unsubscribe;
  }, []);

  useEffect(() => {
    void initialize();
  }, [initialize]);

  if (phase === 'error' && error) {
    return <MobileErrorScreen error={error} onRetry={() => { setError(null); setPhase('loading'); void initialize(); }} />;
  }
  if (phase === 'loading') return <MobileInitScreen />;
  if (phase === 'biometric') return <MobileBiometricScreen />;

  return (
    <>
      <MobileNetworkBanner online={networkOnline} connectionType={connectionType} />
      {children}
    </>
  );
}

function LocalApp() {
  const initialised = useAppStore((s) => s.initialised);
  const initError = useAppStore((s) => s.initError);
  const init = useAppStore((s) => s.init);

  useEffect(() => {
    init();
  }, [init]);

  if (initError) {
    return (
      <div className="flex h-screen w-screen items-center justify-center bg-rc-bg-base">
        <div className="max-w-md text-center space-y-4">
          <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-lg bg-rc-accent-error-bg text-rc-accent-error">
            <TriangleAlert size={24} />
          </div>
          <h1 className="text-lg font-bold text-rc-text-primary">初始化失败</h1>
          <p className="text-sm text-rc-text-secondary break-all">{initError}</p>
          <button
            onClick={() => init()}
            className="px-4 py-2 bg-rc-bg-user-bubble text-rc-text-inverse rounded-lg text-sm font-medium hover:opacity-90 transition-colors"
          >
            重试
          </button>
        </div>
      </div>
    );
  }

  if (!initialised) {
    return (
      <div className="flex h-screen w-screen items-center justify-center bg-rc-bg-base">
        <div className="flex items-center gap-3 text-rc-text-secondary">
          <div className="w-5 h-5 border-2 border-rc-border-primary border-t-rc-text-primary rounded-full animate-spin" />
          <span className="text-sm font-medium">正在初始化...</span>
        </div>
      </div>
    );
  }

  return (
    <ThemeProvider>
      <Layout>
        <div className="flex h-full min-h-0 flex-col bg-transparent">
          <WorkspaceOverview />
          <ChatArea />
          <ChatInput />
        </div>
      </Layout>
      <PermissionModal />
    </ThemeProvider>
  );
}

function App() {
  const nativeRuntime = hasTauriRuntime();
  const nativeMobile = nativeRuntime && isMobileSync();
  const mobileExperience = nativeRuntime && (nativeMobile || isTouchDevice());

  if (!nativeRuntime) {
    return (
      <AppErrorBoundary>
        <MarketingSite />
      </AppErrorBoundary>
    );
  }

  if (shouldUseRemoteMode()) {
    if (nativeMobile) {
      return (
        <AppErrorBoundary>
          <MobileGate>
            <Suspense fallback={<MobileInitScreen />}>
              <MobileRemoteApp />
            </Suspense>
          </MobileGate>
        </AppErrorBoundary>
      );
    }
  }

  if (mobileExperience) {
    return (
      <AppErrorBoundary>
        <MobileGate>
          <LocalApp />
        </MobileGate>
      </AppErrorBoundary>
    );
  }

  return (
    <AppErrorBoundary>
      <LocalApp />
    </AppErrorBoundary>
  );
}

export default App;
