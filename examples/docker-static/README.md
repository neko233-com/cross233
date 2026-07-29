# Docker 服务穿透模板

该模板用 nginx 在容器内监听 `8080`，宿主机仅暴露
`127.0.0.1:18080`，再由 cross233 将服务器的 `60080` 转发到容器。

```bash
docker build -t cross233-static-demo .
docker run --rm --name cross233-static-demo \
  -p 127.0.0.1:18080:8080 cross233-static-demo
```

复制 `client.toml.example`，填写服务器地址和认证密钥后启动：

```bash
cross233-client -c client.toml
```

验证：

```bash
curl -fsS http://127.0.0.1:18080/healthz
curl -fsS http://YOUR_SERVER_IP:60080/
```

停止验证：

```bash
# 先停止 cross233-client，再停止容器
docker stop cross233-static-demo
```

停止 client 后，server 会释放公网监听端口。要暴露已有容器或其他本地
服务，不需要使用本 Dockerfile，只需把 `localAddr` 改为该服务在宿主机
上的监听地址。
