# 图标母版来源 / Icon master provenance

- `tail.png` — PHL 应用图标的主图形（尾鳍）。
  由仓库作者用图像生成工具产出后，经提取轮廓、只保留最大连通区域、
  二值化并重新羽化边缘得到；最终压缩为 **单一颜色 `#4D6BFE` 的平涂模板**
  （构建脚本只读取它的 alpha 通道）。
- `tail-folds.png` — 折面高光转成的内部负空间（DeepSeek 标记中白色腹部线的同类语言）。
  只在 ≥32px 的尺寸使用，小尺寸会丢弃以免糊成一团。

**边界说明**

1. 这不是 DeepSeek 的官方图形，PHL 也不是 DeepSeek 官方产品；
   母版是独立绘制的鲸鱼尾鳍，只借用了官方品牌蓝 `#4D6BFE` 作为配色。
2. 不得用本目录的素材替换或冒充 DeepSeek 官方标识；发布物中应注明「非官方」。
3. 重新生成：`npm run icon`（见 `scripts/make-icon.mjs`）。
