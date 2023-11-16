# auto_tx

## 编译docker镜像
```
docker build -t citacloudcache/auto_tx .
```
## 使用方法

```
$ auto_tx -h
auto_tx 0.1.0
Rivtower Technologies <contact@rivtower.com>

Usage: auto_tx <COMMAND>

Commands:
  run   run this service
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```
# auto_tx

## 编译docker镜像
```
docker build -t citacloudcache/auto_tx .
```
## 使用方法

```
$ auto_tx -h
auto_tx 0.1.0
Rivtower Technologies <contact@rivtower.com>

Usage: auto_tx <COMMAND>

Commands:
  run   run this service
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## 服务接口

### 发送交易

`chain_name`: 指定发送交易的目标链

`request_key`: 交易的标识

`user_code`: 指定发送交易的用户，以该标识从`kms`获取

`to`: 交易中的`to`字段，不填表示创建合约

`data`: 交易中的`data`字段

`value`: 交易中的`value`字段，表示交易发送的原生代币的值，十进制数字的字符串形式，单位`wei`，可选

`timeout`: 目标链类型为`cita`或`cita-cloud`时有效，表示交易超时时间，发送交易任务会在`timeout`内得到结果，上限600s，可选

```
curl -X POST "http://127.0.0.1:4000/api/$chain_name/send_tx" \
     -H "Content-Type: application/json" \
     -H "request_key: $request_key" \
     -d '{
          "user_code": "$user_code",
          "to": "0x1879C8B68c50A4D4eeC9852325d32B60B43f3FbD",
          "data": "0xabcd1234",
          "value": "0",
          "timeout": 300
         }'
```

* 响应：

`hash`: 初始哈希值，在不重试的情况下为上链哈希


```
{
    "code": 200,
    "data": {
        "hash": "0x...",
    },
    "message": "OK"
}
```

### 查询结果

`request_key`: 交易的标识

`user_code`: 指定发送交易的用户，以该标识从`kms`获取

```
curl -X GET "http://127.0.0.1:4000/api/get_onchain_hash" \
     -H "request_key: $request_key" \
     -H "user_code": "$user_code" \
```

* 成功时响应：

`is_success`: 任务的执行状态，`true`表示执行成功，`false`表示失败

`onchain_hash`: 上链的交易哈希

`contract_address`: 合约地址，如果是创建合约交易则会有这个字段


```
{
    "code": 200,
    "data": {
        "is_success": true,
        "onchain_hash": "0x...",
        "contract_address": "0x..."
    },
    "message": "OK"
}
```

* 失败时响应：

`is_success`: 任务的执行状态，`true`表示执行成功，`false`表示失败

`last_hash`: 最后一次重试的交易哈希值

`err`: 错误信息


```
{
    "code": 200,
    "data": {
        "is_success": false,
        "last_hash": "0x...",
        "err": "timeout"
    },
    "message": "OK"
}
```

* 无结果时响应：


```
{
    "code": 500,
    "message": "not found"
}
```
