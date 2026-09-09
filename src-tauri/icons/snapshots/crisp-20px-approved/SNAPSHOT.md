# 快照 / snapshot: crisp-20px-approved

这一版是用户确认「清晰」的图标集，作为回滚基线。**不要随图标一起删除。**

- 小尺寸处理：`--small=crisp`
  - ≤20px：alpha gamma 0.55 + 边缘对比度压缩 0.40–0.60
  - ≤32px：边缘对比度压缩 0.30–0.70
- 生成时间：2026-09-09
- 恢复方式（二选一）：
  1. `npm run icon -- --small=crisp`（脚本内置该模式，重新生成）
  2. `node scripts/restore-icon-snapshot.mjs crisp-20px-approved`（字节级覆盖）
