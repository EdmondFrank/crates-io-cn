## crates-io.cn

基于对象存储的全量 crates.io 镜像

本项目基于 https://github.com/crates-io-cn/crates-io-cn 修改

### Working In Progress

- [x] 缓存反代
- [x] 预热
- [x] 上传新 crate 百度云的BOS
- [x] 接管 crates.io-index 更新
- [ ] 制品搜索
- [ ] 通过 Web API 提供同步状态


### Run
1. 编译
```shell
cargo build --release --features bos
```

2. 初始化配置文件
运行前需要确保.env文件和config目录和可执行文件在同一个目录下
```
cp .env.sample .env
edit .env
```

3. Elasticsearch 搜索同步数据同步支持
```shell
cargo build --release --features "bos search"
```

4. 初始化elasticsearch index 和 mappings
```shell
cargo run --bin init-elastic --features search
```
