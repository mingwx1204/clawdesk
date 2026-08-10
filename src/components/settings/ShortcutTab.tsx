import { Label } from '@/components/ui/label';
import { Input } from '@/components/ui/input';
import { useSettingsStore } from '@/store/useSettingsStore';

export function ShortcutTab() {
  const { settings, update } = useSettingsStore();
  return (
    <div className="space-y-4">
      <div className="space-y-2">
        <Label>全局唤起 / 隐藏快捷键</Label>
        <Input
          className="w-64"
          value={settings.globalShortcut}
          onChange={(e) => void update({ globalShortcut: e.target.value })}
          placeholder="Ctrl+Shift+O"
        />
        <p className="text-xs text-muted-foreground">格式示例：Ctrl+Shift+O、Alt+Space。修改后立即生效。</p>
      </div>
    </div>
  );
}
