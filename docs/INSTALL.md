# 安装部署指南

**[English](INSTALL.en.md)** · 返回 [README](../README.md)

> **这份文档给谁**：第一次给 reMarkable Paper Pro Move 装这套增强的人、想卸载的人、固件升级（OTA）后要恢复功能的人。
> 装：按顺序读「适用范围 → 装之前 → 安装 → 装完之后」。卸：读「卸载」。出问题：查「常见问题」。升级固件：读「固件升级（OTA）之后」。
> 脚本内部怎么写、全部参数与环境变量、怎么本机测试，见 `packaging/` 目录下各脚本自己的头注与 `-h` 输出（本文不重复）。

## 适用范围

**reMarkable Paper Pro Move（imx93-chiappa），固件 3.28.0.172**。这是目前唯一在真机上验证过的版本，
安装脚本动手前会自动核对（见「固件安全门」）。其它固件版本、其它 reMarkable 型号都没验证过；强装的话，
界面补丁可能定位错，轻则某个功能不生效，重则影响设备正常使用。

## 装之前

### 设备上：4 样东西要你手动装好

它们属于 reMarkable 第三方生态，不属于这个仓库，`install-all.sh` **不会**代装。缺了的话，对应步骤会报错或跳过，
并告诉你该跑哪条命令。vellum（设备上的包管理器）本身怎么装、KOReader 怎么侧载，看 vellum 和社区自己的文档。

| # | 在设备上做什么 | 它是什么 | 缺了会怎样 |
|---|---|---|---|
| 1 | `vellum add xovi` | [xovi](https://github.com/asivery/xovi)：扩展加载框架 | 插件类功能都靠它，相关步骤直接失败 |
| 2 | `vellum add qt-resource-rebuilder` | 界面补丁（qmd）加载器 | 界面补丁全部不装（不算失败）：侧栏入口、字体菜单、回收站/新建文件夹代理、漫画页边距代理、阅读器单击翻页/日漫翻页规则。其余不受影响（汇总里怎么显示见问题②） |
| 3 | `vellum add appload`（**≥ 0.6.0**） | 第三方 App 加载器 | 侧栏 KOReader 入口不出现（`sidebar-entry` 步自动跳过） |
| 4 | 经 appload 侧载 KOReader | 第二个阅读器 | `koreader-serve` 只管理已装好的 KOReader，不负责装 |

可选：第三方 **WeRead** app（微信读书 reMarkable 版）。装了的话，`sidebar-entry` 会自动多加一项「WeRead」入口；没装不影响任何功能。

### 电脑上：编译环境和 ssh

脚本在你的电脑上把程序编好，再经 ssh 装到设备上，所以电脑需要：

| 需要什么 | 用在哪 | 缺了会怎样 |
|---|---|---|
| Rust（`cargo`）+ `rustup target add aarch64-unknown-linux-musl` + `aarch64-linux-gnu-gcc` | 交叉编译九个网页服务和电池刺客（`shelf/build.sh`、`deploy-battop.sh`） | `shelf`、`battop` 步失败 |
| Qt 的 `rcc`（Qt 开发包里带） | 把侧栏图标打成资源包（`deploy-sidebar-entry.sh`） | `sidebar-entry` 步报错 |
| 可选：[asivery/xovi](https://github.com/asivery/xovi) 的源码 clone（`XOVI_DIR` 指向它） | 重新编译 `hl-snap` / `hw-stroke` 两个插件 | 不影响：仓库里已提交编好的 `.so`，编不了就用它 |
| **能免密 ssh 登录设备 root** | 所有步骤（脚本不会停下来问密码） | 动手前就报错并给排查步骤。没配过先跑 `ssh-copy-id root@10.11.99.1` |

## 安装

### 推荐顺序

1. **确认固件版本**：设置里看系统版本，目前只有 3.28.0.172 验证过。
2. **按顺序手动装好设备上那 4 样**（xovi → qt-resource-rebuilder → appload → 侧载 KOReader；可选 WeRead）。装完 appload 先确认侧栏出现了原生「AppLoad」图标（见问题①）。
   ⚠ **appload 和 WeRead 两步之间隔几分钟**：WeRead 每次进出都让 xochitl 停起一次，短时间内停起太多次会触发设备的重启保护，整机重启（2026-09-11 真机踩过，见问题③）。装完 appload 要让它生效，直接整机重启设备（`reboot`），别 `systemctl restart xochitl`（见问题①）。
3. **先预演**（只在电脑上跑，不连设备）：
   ```sh
   git clone https://github.com/bbq191/rm-tweak.git
   cd rm-tweak/packaging
   sh install-all.sh --dry-run          # 只打印将执行哪些步骤；可以加 --skip 看跳过后的样子
   ```
   预演**不**核对固件、**不**检查设备，只证明命令行写对了。
4. **正式安装**（USB 连电脑，设备地址默认 `10.11.99.1`）：
   ```sh
   sh install-all.sh 10.11.99.1
   ```
   脚本先确认 ssh 能通、固件在白名单里、设备状态正常（见「装前自动检查」），再按下表逐步执行。第一次会先编译，要等一会儿。
5. **看收尾汇总**：分四栏——「已安装」「已跳过（--skip）」「已跳过（前置条件不满足，非失败）」「失败」。第三栏会写明原因（例如设备没装 appload、dm-verity 开着装不进 `/usr`）；"跳过"不等于"失败"，容易漏看（见问题①②）。有失败项就照报错处理，其余已经装好；整条重跑也安全（脚本全部幂等，内容没变就不会再重启设备）。
6. **登录网页、改密码、装证书**：见「装完之后」。
7. **肉眼确认侧栏入口**（如果这步没被跳过）：回设备主界面，看侧栏 KOReader 入口在不在、能不能点开。这一步脚本替你确认不了。

### 每一步装了什么

表里的**步骤名**可以用在 `--skip` 里；对应脚本 `packaging/deploy-<步骤名>.sh <host>`（`shelf` 步是 `deploy.sh`）也能单独运行。

| 步骤名 | 做什么 | 前置 |
|---|---|---|
| `chrony-cn` | 校时服务器换成国内能连上的（阿里云、腾讯云等） | — |
| `chrony-boot-wakelock` | 开机头一小段（同步成功就放，最多 120 秒）不让设备自动休眠，免得打断第一次校时 | — |
| `timezone-cn` | 默认时区设为 Asia/Shanghai | — |
| `battop` | 电池耗电诊断的采样服务；装完启动，但**不随开机自启**（有意的，见问题⑥）。dm-verity 开着且以前没装过时，程序放好了但服务单元进不了 `/usr`，汇总记"前置条件不满足" | — |
| `wifi-watch` | WiFi 假死看护：检测到断链自动重连，并固定 2.4G 频段、关闭 WiFi 省电 | — |
| `xovi-persist` | 开机后自动让 xovi 重新生效，重启设备后不用手动补 | xovi |
| `hl-snap` | 荧光笔划中文"划哪吸哪"，不再"划一小段吸整行"；只落盘 | xovi |
| `handwriting-stroke` | 按笔尖角度和运笔速度优化手写笔画粗细（默认关，网页「管理 → 实验室」里开）；只落盘 | xovi |
| `sidebar-entry` | 侧栏直达「KOReader」；装了 WeRead 自动多一项；只落盘 | qt-resource-rebuilder + appload（见问题①） |
| `shelf` | 九个网页服务：网关、书（book / koreader）、字体与壁纸（font / wallpaper）、笔记四服务（ink / transcribe / mind / note）；附带的五个界面补丁（字体菜单、回收站代理、建文件夹代理、漫画页边距代理、阅读器翻页）只落盘 | 补丁需要 qt-resource-rebuilder，缺了只跳过补丁、服务照装 |
| `xovi-apply` | 上面"只落盘"的东西都就位后，**有改动（或 xovi 还没生效）才整机重启一次**让它们生效（约 20–60 秒，会打断阅读；没改动就不重启）。重启回来后自动跑一遍 `verify-on-device.sh` 核对 | — |

"只落盘"的意思是：文件先放到位，暂不生效，最后由 `xovi-apply` 统一整机重启一次。这样避免短时间内反复重启。
失败的步骤不会自动重试，也不会被悄悄跳过。

## 装完之后

**先看核对结果**：最后一步整机重启后，脚本会等设备回来并自动跑 `verify-on-device.sh`（`CJ_APPLY_VERIFY=0` 可关）。它只读检查设备，共 9 类 44 项（固件、xochitl/xovi、扩展、界面补丁、各服务、本次开机告警、端口、`/usr` 单元、磁盘），逐项给 ✓/⚠/✗，有 ✗ 时退出码非 0。没有触发重启、或以后单独部署某一步之后，可以自己在 `packaging/` 下跑 `sh verify-on-device.sh <host>`。每项含义见脚本头注。

浏览器打开 `https://10.11.99.1/`（同一 WiFi 下也可以用 `https://shelf.local/`；安卓不认 `.local` 域名，要用设备的 IP）。

- **改密码**：默认密码 `shelf`，第一次登录会强制跳到改密页。
- **登录限速**：同一个 IP 在 60 秒内输错 5 次会被暂时锁住；换一台设备（不同 IP）不受影响。
- **装证书**：浏览器会提示证书不受信任，因为它是设备自己签的。登录页有「下载 CA 证书」（地址 `https://<设备>/ca.crt`），装进手机或电脑的信任列表一次，以后就不提示了。iOS 装完还要在"设置 → 通用 → 关于本机 → 证书信任设置"里打开完全信任。临时用也可以点「高级 → 继续访问」。
- **从 2026-09-24 之前的版本升级的**：网关第一次启动会自动换一张新 CA（新 CA 只能给局域网名字和内网 IP 签证书；旧文件改名为 `*.bak-<时间>` 留在设备上，不删）。**每台手机和电脑都要重装一次新证书，并删掉旧的那张**——旧 CA 没有这层限制，只装新的不删旧的，风险还在。确认新证书能用后，可以删掉设备 `~/.config/shelf/tls/` 里的 `*.bak-*`。
  - 已知限制：用不在范围内的地址访问（例如运营商分配的 100.64.x.x、公网 IP，或你自己改过的 mDNS 名字），证书会对不上、浏览器报错。

## 常用选项

### 命令速查

在 `packaging/` 目录下执行；`<host>` 缺省 `10.11.99.1`。每个脚本都能 `-h` 看用法；参数写错一律退出码 2，不会去连设备。

| 命令 | 参数 | 作用 |
|---|---|---|
| `sh install-all.sh [host]` | `--dry-run` | 只打印计划，不连设备 |
| | `--skip a,b` | 跳过指定步骤（写错名字只警告，并列出已知步骤名） |
| | `--force` | 固件不在白名单也装（见「固件安全门」） |
| | `--force-apply` | 最后一步无论有没有改动都整机重启一次 |
| `sh uninstall-all.sh [host]` | `--dry-run` / `--skip a,b` / `--purge` | 见「卸载」 |
| `sh deploy.sh [host]` | `--only a,b` · `--password 新密码` · `--no-systemd` | 只装或更新网页服务（见「只装一部分」） |
| `sh deploy-xovi-apply.sh [host]` | `--force` | 单独让"只落盘"的内容生效 |
| 其余 `deploy-<步骤名>.sh [host]` | `-h` | 单独跑某一步 |
| `sh verify-on-device.sh [host]` | `--json` · `--dump` · `--from 文件` | 只读核对设备现状（见「装完之后」） |

### 装前自动检查

`install-all.sh`（`--dry-run` 除外）动手前做三件事，任何一件不过就**一个步骤都不执行**，设备上什么都没改：

1. **ssh 能不能通**：连不上就报错并给排查步骤（设备休眠或没插 USB；IP 不对；设备的 host key 变了；没配免密）。整轮只查这一次，后面各步骤不再重复。
2. **固件安全门**：见下。
3. **设备预检**（只读）：必须是 root、`/home` 可写；`/home` 剩余空间**不到 50MB 拒装、不到 200MB 警告**；再报告 xovi、qt-resource-rebuilder、appload 装没装，xovi 是否已在 xochitl 里生效。缺什么只是提前告诉你，对应步骤自己会报错或跳过。

### 固件安全门

装之前脚本读设备上 `/usr/bin/xochitl` 的 sha256，对照仓库里的 `packaging/firmware-allowlist.txt` 和你电脑上的 `firmware-allowlist.local.txt`：对得上才继续，否则默认拒装。用哈希而不是版本号，是因为界面补丁按字节定位，同一个版本号的热修补丁也可能挪动内部布局。

确认这台设备的固件就是你要装的、只是哈希没登记，加 `--force`。当前哈希会追加到**你电脑上的** `firmware-allowlist.local.txt`（不进 git），以后同一固件不用再加。

```sh
sh install-all.sh 10.11.99.1 --force
```

### 重复运行：什么时候才重启

重跑 `install-all.sh` 是安全的，而且不会每次都闪屏。只有真的写入了新内容时，脚本才在设备上留一个"待生效标记"（放在内存盘 `/run/cangjie-pending-apply/`，设备重启即清）；最后一步看标记决定要不要重启。

| 情形 | 最后一步 `xovi-apply` 怎么做 |
|---|---|
| 这轮有内容变了（首次安装、更新了插件或界面补丁） | **整机重启一次**（先打印"将打断阅读"，等 5 秒；约 20–60 秒回来，回来后自动核对）。2026-09-25 起不再单独重启 xochitl：它退出时有概率崩溃、再由系统整机重启，不如直接干净地重启 |
| 更新了插件 `.so`，而 xochitl 正在用旧版 | 新版先放进待换入区，重启前换进去（见下图；换入到排上重启这一段电脑断线或按 Ctrl-C 也会做完） |
| 什么都没变，xovi 已生效 | 不重启 |
| 什么都没变，但设备刚重启过、xovi 还没生效 | 装了 xovi 持久化：整机重启（开机时自动恢复）；没装：执行 `xovi/start` |
| 待换入区里有新版插件，设备先被你手动重启了 | 开机时 `xovi-reenable` 会先把它换进去，不用再跑什么 |
| 加了 `--force-apply` | 无论如何都整机重启一次 |
| 上一轮用 `--skip xovi-apply` 跳过了 | 标记还在，补一句 `sh deploy-xovi-apply.sh <host>` 即可 |

**单独跑某一步也一样**：`deploy-hl-snap.sh`、`deploy-handwriting-stroke.sh`、`deploy-sidebar-entry.sh` 单独运行时，有改动就整机重启一次；文件和设备上已装的逐字节相同、也没有别的待生效改动、xovi 已生效，就**不重启**。判据与最后一步 `xovi-apply` 是同一个。

### 只装一部分

```sh
sh install-all.sh 10.11.99.1 --skip chrony-cn,timezone-cn,xovi-persist    # 跳过指定步骤
sh deploy.sh 10.11.99.1 --only book,koreader --password '新密码'            # 只装书架的两个服务，顺便设网关密码
```

`--only` 可以写的名字：`gateway book koreader font wallpaper ink transcribe mind note`。网关总会装；写了别的名字会报错退出。
`--password` 经 ssh 标准输入传到设备上的临时文件，用完就删，不会出现在命令行和进程列表里（只有经 `deploy.sh` 传时才这样，见「已知限制」）。
`deploy.sh` 推送前会核对要装的程序都编好了；缺了会直接报错，提示先在仓库根目录跑 `sh shelf/build.sh`。

## 卸载

`uninstall-all.sh` 和安装用同一张步骤表，按**相反顺序**执行（后装的先卸）。建议先预演：

```sh
cd packaging
sh uninstall-all.sh 10.11.99.1 --dry-run          # 只打印计划，不连设备、不删东西
sh uninstall-all.sh 10.11.99.1                    # 卸全部
sh uninstall-all.sh 10.11.99.1 --skip shelf       # 跳过某步
sh uninstall-all.sh 10.11.99.1 --purge            # 另外删掉电池刺客的程序和历史采样数据
```

**会做什么**：停用并删掉装过的服务、插件、界面补丁，以及部署时推到设备上的安装包目录（只删认识的文件，目录里有别的东西就留着）。卸插件时连待换入区里还没换进去的新版也一并撤掉（2026-09-24 之前不撤，下一次部署会把刚卸掉的插件又装回来）。

**默认保留**：母版库、配置、证书、字体/壁纸池、`cangjie-backups/` 里的备份。`--purge` 只管电池刺客，不碰书架数据；要连书架数据一起删，先 `--skip shelf` 卸别的，再在设备上跑 `shelf-uninstall --purge`。

**不会做什么**：
- `chrony-cn`、`timezone-cn` 是改配置、`xovi-apply` 只是个动作，都不卸。改之前的备份在设备 `cangjie-backups/` 里，要还原自己取。
- vellum、xovi、qt-resource-rebuilder、appload 和 KOReader 不是本项目装的，也不卸。
- 卸载**不重启**设备。已经加载的插件和界面补丁要等下次重启才真正停用。想马上停：在设备上 `reboot`（整机重启）。**别** `systemctl restart xochitl`：xochitl 自己退出时有概率崩溃，崩了系统会走应急路径整机重启（2026-09-25 真机多次），不如直接干净地重启。

**系统分区只读校验（dm-verity）开着时**：`/usr` 下的服务单元删不掉（脚本遇到 verity 一律不写 `/usr`，写 `/usr` 曾经让设备回滚变砖）。这时卸载脚本会如实提示，并**保留**这些单元要用的程序，免得重启后单元找不到程序、反复失败。等设备可写后再跑一次 `uninstall-all.sh` 就能收尾。

卸载全流程 2026-09-25 在真机整轮跑过：8 步逆序全部成功，书架/笔记数据与配置都保留，卸载期间 xochitl 没有重启。dm-verity 下保留程序、`--purge` 这两支仍只有本机模拟。

## 固件升级（OTA）之后

这是 OTA 恢复的**权威说明**，其它文档都链接到这里。

**升级本身不会丢 `/home` 的数据，但升完要重跑一遍安装，功能才回来。** 本项目有意不在启动路径上留任何东西（xovi 的加载配置在 `/etc` 的内存层、服务单元在 `/usr`），所以新固件总是以纯原厂状态起来。

### 推荐流程

1. （升级前，可选）把与新固件不兼容的 xovi 插件（例如旧版 appload）挪出 `extensions.d/`，放到 `/home/root/xovi-disabled/`。**绝不留在 `extensions.d/` 里**：xovi 会把那个目录下任何文件都当插件加载。
2. 升级完成后，**在设备旁手动**跑 `xovi/rebuild_hashtable`（要输 root 密码，脚本不代做）。它是界面补丁重新生效的前提。
3. 在电脑上：`cd packaging && sh install-all.sh <设备IP>`。新固件的哈希一般不在白名单里，确认版本无误后加 `--force`。OTA 后 xovi 没有生效，所以最后一步会整机重启一次，开机时由刚装回的 `xovi-reenable` 恢复 xovi，回来后自动核对。
4. 看收尾汇总，浏览器打开网关确认。appload 要 ≥ 0.6.0（旧版先 `vellum upgrade appload` 并整机重启），它不在安装脚本里。

### 逐项对照

| 内容 | 位置 | OTA 后 | 怎么恢复 |
|---|---|---|---|
| 母版库、KOReader 配置、字体与壁纸池、证书、网关密码、休眠屏设置、`cangjie-backups/`、电池采样历史 | `/home` | 保留 | 不用管 |
| 各网页服务的程序（`~/.local/bin`） | `/home` | 保留 | 不用管 |
| `hl-snap` / `hw-stroke` 插件、侧栏入口与字体菜单/回收站/建夹/漫画边距/阅读器翻页的界面补丁 | `/home`（`extensions.d/`、`exthome/`） | 文件还在，但要重建 hashtable 才生效 | 第 2 步，再跑 `install-all.sh` |
| 各网页服务与 `shelf.target` 的服务单元 | `/usr` | **被冲掉** | `shelf` 步（或单独：`SHELF_NO_BUILD=1 sh deploy.sh <设备IP>`） |
| `xovi-reenable.service`（开机自动让 xovi 生效） | `/usr` | **被冲掉** | `xovi-persist` 步 |
| `chrony-boot-wakelock.service` | `/usr` | **被冲掉** | `chrony-boot-wakelock` 步 |
| `battop.service`（数据在 `/home`） | `/usr` | 单元**被冲掉** | `battop` 步（装完启动，不自启） |
| `wifi-watch.service`（脚本在 `/home`） | `/usr` | 单元**被冲掉** | `wifi-watch` 步 |
| 国内校时服务器、默认时区 | `/etc` | **被冲掉** | `chrony-cn` / `timezone-cn` 步 |
| appload ≥ 0.6.0 | `/home`（xovi 插件） | 看 appload 有没有被重装 | 设备上 `vellum upgrade appload`，然后整机重启 |

**风险分层**：书架这层只用 xochitl 的网页上传接口和系统标准组件，换固件重装就回来；字体菜单这类界面补丁依赖 xochitl 内部 QML，大版本升级常要重新适配；KOReader 本体不受影响，但侧栏入口靠 appload，每个固件大版本都要确认 appload 已经支持。

**"裸机恢复"要多查一步**：OTA 本身不删 `/home`，但如果设备做过更彻底的重置，`/home` 下的插件和程序可能也没了（2026-09-09 真机踩过）。重跑 `install-all.sh` 前先确认它们还在。

## 常见问题

下面都是有明确触发条件的已知坑，不是随机故障。编号 ①–⑧ 在上文被引用。

| # | 现象 | 原因 | 怎么办 |
|---|---|---|---|
| ① | 侧栏没有 KOReader/WeRead 入口；汇总里 `sidebar-entry` 列在"已跳过（前置条件不满足）" | appload ≤ 0.5.3 不支持 3.28 的界面，自己的启动器建不起来。**不会**导致装不上或 xochitl 起不来，只是这一个功能不生效 | `vellum list --installed \| grep appload` 看版本，旧版就 `vellum upgrade appload`（0.6.0 起支持 3.28，2026-09-21 真机验证过）。**升级 appload 后整机重启，不要 `systemctl restart xochitl`**：停止 xochitl 时它有概率在退出途中崩溃，触发整机自动重启（2026-09-21 就是这样） |
| ② | 字体菜单、回收站/新建文件夹、漫画页边距、阅读器单击翻页、侧栏入口这几个**同时**没有 | 它们共用同一个前置 qt-resource-rebuilder。没装时：`sidebar-entry` 在汇总里列进"已跳过（前置条件不满足）"；其余几个是 `shelf` 步里附带的补丁，**不单列**——`shelf` 仍算"已安装"，只在这一步的输出里有一行"无 qt-resource-rebuilder 目录…跳过字体菜单/回收站/建夹 qmd" | `vellum add qt-resource-rebuilder` 后重跑 `install-all.sh` |
| ③ | 短时间内 xochitl 反复停起后，设备整机重启了一次 | xochitl 服务设置了 10 分钟内最多重启 4 次，不管谁触发的都算：手动 `systemctl restart xochitl`、`vellum add appload`、WeRead 每次进出。2026-09-11 真机上连续两次重启就触发过一次整机重启——**设备自己重启后恢复正常，不是变砖** | 部署脚本 2026-09-25 起改为整机重启，不再计入这个次数。**来回折腾 appload/WeRead 时**，每次间隔几分钟 |
| ④ | 固件安全门拒装 | 设计如此：版本号相同不保证内部布局没变 | 先确认设备固件就是你验证过的那份，再 `--force` |
| ⑤ | 装到最后设备重启了一次 | `xovi-apply` 让改动生效：2026-09-25 起一律**整机重启**（约 20–60 秒回来），不再单独重启 xochitl——单独重启它有概率在退出时崩溃、再由系统整机重启。只有这轮真的有改动、或 xovi 还没生效时才会重启 | 正常现象，装的时候别操作设备；脚本会等设备回来并自动跑一遍 `verify-on-device.sh` 核对。不想被打断就 `--skip xovi-apply`，稍后再跑 `sh deploy-xovi-apply.sh <host>`。**自己手动让它生效时**：直接 `reboot`；**绝不**手动跑 `xovi/start`（xovi 已生效时它会让 xochitl 崩溃、整机自动重启，2026-09-20 真机事故） |
| ⑥ | 重启设备后电池刺客没在跑 | **有意不开机自启**：2026-08-29 它的采样曾触发内核死锁冻死整机，根因没彻底排除 | 网页「管理 → 系统增强」里打开电池刺客开关（开了才出现「电池刺客」数据页），或 `systemctl start battop` |
| ⑦ | 装之前就报错退出：`连不上 root@…` / `只剩 N MB 可用` / `需要 root` / `固件不在白名单` | 装前自动检查在拦，设备上什么都没改 | 连不上：按报错里的步骤排查（休眠/没插 USB → IP → host key → 免密）；空间不足：清理 `/home/root` 和 `cangjie-backups/` 后重试；固件：见 ④ |
| ⑧ | 最后一步报"设备没能排上整机重启……改动尚未生效"，这一步记失败 | 设备上的 `systemctl reboot` 命令本身失败了。文件已经换好，但 xochitl 还在用旧的；脚本已把"待生效"标记补回去（2026-09-25 起，此前会白等设备重启再报成功）。这一支只在本机模拟过 | 在设备上手动 `reboot`，回来后跑 `sh verify-on-device.sh <host>`；或者稍后重跑 `sh deploy-xovi-apply.sh <host>`，它会再试一次 |

### 其它排障

- 先看 `install-all.sh` 的收尾汇总，定位哪一步失败；对应 `packaging/deploy-*.sh` 的开头注释写了这一步做什么、常见失败原因。
- 网页「管理」页能直接看到插件是否真的加载进了 xochitl（"已加载 / 未加载"）。开关开着但显示"未加载"，说明插件没装上或装完还没整机重启。「管理 → 设备健康」能看到更全的状态（各服务、扩展、上次开机日志）。
- 不碰真机就想确认脚本没被改坏：`bash packaging/tests/run_sim_tests.sh`（本机模拟，314 项断言）。它代替不了真机验证。

### 备份与幂等（一句话版）

所有安装脚本都可以重复跑；覆盖设备上已有文件前先备份到 `/home/root/cangjie-backups/`（**绝不**放 `extensions.d/`），只留最近 5 份，内容没变就不备份也不动；写 `/usr` 前先检查 dm-verity，开着就跳过。

## 已知限制

- **哪些在真机上跑过、哪些没有**：真机整轮跑过的有 `install-all.sh`（2026-09-22、09-24、09-25 各一次）和 `uninstall-all.sh`（2026-09-25）；"换入新版 `.so` → 整机重启 → 开机自动恢复 → 自动核对"2026-09-25 真机复核通过（`verify-on-device.sh` 43✓）。**只有本机模拟、没在真机走过的**：卸载在 dm-verity 下保留程序、卸载时撤掉待换入区、"什么都没变就不重启"这一支（含单独部署）、换入关键区忽略断连信号、汇总的"前置条件不满足"栏，以及 2026-09-25 下午第四轮审计改的全部安装脚本行为（整机重启失败的处理、dm-verity 下的电池刺客、部署时 ssh 往返合并等）。上机时一步一确认：先 `--dry-run`，再单步或 `--skip` 试跑。
- **写 `/usr` 仍靠"先查 dm-verity + 限时读写窗口"两道防线**，不是完全不碰 `/usr`；历史上写 `/usr` 触发过回滚变砖（2026-08-16）。
- **在设备上直接跑 `shelf/install.sh --password 明文` 时，密码会短暂出现在设备的进程列表里**；经电脑上的 `deploy.sh --password` 传则不会。
- 卸载不还原 `chrony-cn` / `timezone-cn`，没有"一键回到装之前"。

## 这套安装器不做什么

- **不装 vellum / xovi / qt-resource-rebuilder / appload，不侧载 KOReader**：见「装之前」。
- **不装中文输入法**：那条功能线的源码已移出本仓库（见 [README](../README.md#历史与范围)），不随本安装器分发。
- **不升级 appload**：3.28 固件要 ≥ 0.6.0，旧版请自己升级并整机重启（见问题①）。
