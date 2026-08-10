import { memo, useEffect, useState } from 'react';
import { ArrowLeft, Settings, Cpu, Keyboard, Bot, Dna, Brain, Palette, Puzzle, Info } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { GeneralTab } from '@/components/settings/GeneralTab';
import { ModelTab } from '@/components/settings/ModelTab';
import { ShortcutTab } from '@/components/settings/ShortcutTab';
import { BotPlatformTab } from '@/components/settings/BotPlatformTab';
import { PluginStoreTab } from '@/components/settings/PluginStoreTab';
import { EvolutionTab } from '@/components/settings/EvolutionTab';
import { MediaGenTab } from '@/components/settings/MediaGenTab';
import { AboutTab } from '@/components/settings/AboutTab';
import { MemoryTab } from '@/components/settings/MemoryTab';

const TABS = [
  { id: 'general', label: '通用', icon: Settings },
  { id: 'model', label: '模型', icon: Cpu },
  { id: 'shortcut', label: '快捷键', icon: Keyboard },
  { id: 'botplatform', label: 'Bot', icon: Bot },
  { id: 'evolution', label: '进化', icon: Dna },
  { id: 'memory', label: '记忆', icon: Brain },
  { id: 'mediagen', label: '媒体', icon: Palette },
  { id: 'plugins', label: '插件', icon: Puzzle },
  { id: 'about', label: '关于', icon: Info },
];

/** 设置全屏页面 */
export const SettingsDialog = memo(function SettingsDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [tab, setTab] = useState('general');
  const [exiting, setExiting] = useState(false);

  useEffect(() => { if (open) { setTab('general'); setExiting(false); } }, [open]);

  const handleClose = () => {
    setExiting(true);
    setTimeout(() => { onClose(); setExiting(false); }, 150);
  };

  // ESC 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') handleClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []); // eslint-disable-line

  if (!open && !exiting) return null;

  return (
    <div className={`fixed inset-0 z-50 flex flex-col bg-background ${exiting ? 'animate-fade-out' : 'animate-slide-in-right'}`}>
      <div className="flex h-11 shrink-0 items-center gap-3 border-b border-border/40 px-4 acrylic">
        <Button variant="ghost" size="icon" className="h-8 w-8" onClick={handleClose}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <span className="text-sm font-medium">设置</span>
      </div>
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {/* 标签卡片导航 */}
        <div className="mx-4 mt-3 flex flex-wrap gap-2">
          {TABS.map((t) => (
            <button
              key={t.id}
              className={cn(
                'flex items-center gap-2 rounded-xl px-3.5 py-2 text-sm font-medium transition-all hover-lift',
                tab === t.id
                  ? 'bg-primary/15 text-primary shadow-sm ring-1 ring-primary/20'
                  : 'bg-card text-muted-foreground hover:bg-accent hover:text-foreground',
              )}
              onClick={() => setTab(t.id)}
            >
              <t.icon className="h-4 w-4" />
              {t.label}
            </button>
          ))}
        </div>
        {/* 内容区 */}
        <div className="min-h-0 flex-1 overflow-y-auto px-6 py-4">
          {tab === 'general' && <GeneralTab />}
          {tab === 'model' && <ModelTab />}
          {tab === 'shortcut' && <ShortcutTab />}
          {tab === 'botplatform' && <BotPlatformTab />}
          {tab === 'evolution' && <EvolutionTab />}
          {tab === 'memory' && <MemoryTab />}
          {tab === 'mediagen' && <MediaGenTab />}
          {tab === 'plugins' && <PluginStoreTab />}
          {tab === 'about' && <AboutTab />}
        </div>
      </div>
    </div>
  );
});
