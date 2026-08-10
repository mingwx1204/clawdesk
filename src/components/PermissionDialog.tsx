import { memo } from 'react';
import { ShieldAlert } from 'lucide-react';
import { usePermissionStore } from '@/store/usePermissionStore';
import { useSettingsStore } from '@/store/useSettingsStore';
import {
  AlertDialog, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { Button } from '@/components/ui/button';

/** 全局权限确认弹窗：工具/插件敏感操作前征求用户同意 */
export const PermissionDialog = memo(function PermissionDialog() {
  const { pending, answer } = usePermissionStore();
  const { update } = useSettingsStore();

  if (!pending) return null;

  const allowAll = () => {
    // 切换为「全部允许」模式并放行当前请求
    void update({ permissionMode: 'allow_all' });
    answer(true, false);
  };

  return (
    <AlertDialog open>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle className="flex items-center gap-2">
            <ShieldAlert className="h-5 w-5 text-yellow-500" />
            权限请求
          </AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="space-y-2">
              <p>ClawDesk 想要执行以下操作：</p>
              <div className="rounded-lg border border-border bg-muted p-3">
                <p className="text-sm font-medium text-foreground">{pending.title}</p>
                {pending.detail && (
                  <p className="mt-1 break-all font-mono text-xs text-muted-foreground">{pending.detail}</p>
                )}
              </div>
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter className="flex-col gap-2 sm:flex-row">
          <Button variant="outline" onClick={() => answer(false, false)}>拒绝</Button>
          <Button variant="secondary" onClick={() => answer(true, true)}>允许（本次会话不再询问此类操作）</Button>
          <Button onClick={allowAll}>全部允许（不再询问）</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
});
