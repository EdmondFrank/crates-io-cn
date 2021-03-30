
## crates-io.cn

基于对象存储的全量 crates.io 镜像

本项目基于 https://github.com/crates-io-cn/crates-io-cn 修改

### Working In Progress

- [x] 缓存反代
- [x] 预热
- [x] 上传新 crate 百度云的BOS
- [x] 接管 crates.io-index 更新
- [x] 制品搜索

### 部署/运行

我的环境：
rustc 1.50.0 (cb75ad5db 2021-02-10)
cargo 1.50.0 (f04e7fab7 2021-02-04)


1. Rust/Cargo 安装

参考：https://www.rust-lang.org/learn/get-started

2. 编译
```shell
cargo build --release --features "bos search sync"
```
> 参数说明：
--release: 编译成release版本
--features: 支持特性，条件编译选项
    1. bos：支持百度云对象存储
    2. search: 支持elasticsearch 搜索
    3. sync: 全量同步

3. 初始化配置

3.1 执行文件所在目录结构：

```
tree .
.
├── config
│   └── log4rs.yml   # 日志level
├── crates-io-cn     # 主程序
└── init-elastic     # 初始化elasticsearch索引

1 directory, 3 files
```

运行前需要确保.env文件和config目录和可执行文件在同一个目录下
```
cp .env.sample .env
vim .env
＃ continue editting the .env file
```


3.2 本地索引库的 git remote 配置
```
local	https://gitee.com/huawei_mirror_group/crates.io-index.git (fetch)
local	https://gitee.com/huawei_mirror_group/crates.io-index.git (push)
origin	https://github.com/rust-lang/crates.io-index.git (fetch)
origin	https://github.com/rust-lang/crates.io-index.git (push)
```

4. 运行

可以使用 -h 参数来列出支持的选项

*不需要全量同步的时候，直接启动即可，会自动增量同步*
```
./crates-io-cn
```

*需要全量同步的时候，使用 -s 参数，会遍历索引目录和文件进行全量同步*
```
./crates-io-cn -s
```
