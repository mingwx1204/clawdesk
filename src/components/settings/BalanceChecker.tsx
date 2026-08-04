import { Button } from '@/components/ui/button';
import { useState } from 'react';
import { fetchBalance } from '@/lib/backend';

/** 余额/连接同步小组件（DeepSeek 等支持 /user/balance 的平台） */
export function BalanceChecker({ apiBase, apiKey }: { apiBase: string; apiKey: string }) {
  const [state, setState] = useState<{ loading: boolean; text: string; ok: boolean }>({ loading: false, text: '', ok: false });

  const check = async () => {
    setState({ loading: true, text: '查询中…', ok: false });
    try {
      const b = await fetchBalance(apiBase, apiKey);
      setState({
        loading: false,
        ok: b.available,
        text: `余额 ${b.totalBalance} ${b.currency}${b.available ? '（可用）' : '（不可用）'}`,
      });
    } catch (e) {
      setState({ loading: false, ok: false, text: String(e).slice(0, 60) });
    }
  };

  return (
    <div className="flex items-center gap-2">
      <Button variant="outline" size="sm" className="h-7 text-xs" disabled={state.loading || !apiKey} onClick={() => void check()}>
        同步余额
      </Button>
      {state.text && (
        <span className={`text-[11px] ${state.ok ? 'text-green-400' : 'text-muted-foreground'}`}>{state.text}</span>
      )}
    </div>
  );
}

export function uid(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 9)}`;
}
