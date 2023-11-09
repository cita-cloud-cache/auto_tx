import requests
import time

# target = "http://192.168.120.9:3000/"
# chain_name = "citacloud-shuwenlian"

target = "http://127.0.0.1:3000/"
chain_name = "cita-cmc"

# send tx
url = target + "api/" + chain_name + "/send_tx"

request_key = "auto_tx_test"
user_code = "did:bid:zf26MwL8MVDzjSQb6UMsam4r4pVqPzP93"

headers = {"Content-Type": "application/json", "request_key": request_key, "user_code": user_code}

json = {
    # "to": "0x1879C8B68c50A4D4eeC9852325d32B60B43f3FbD",
    # "data": "0xabcd1234",
    'data': '608060405234801561001057600080fd5b5060f58061001f6000396000f3006080604052600436106053576000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff16806306661abd1460585780634f2be91f146080578063d826f88f146094575b600080fd5b348015606357600080fd5b50606a60a8565b6040518082815260200191505060405180910390f35b348015608b57600080fd5b50609260ae565b005b348015609f57600080fd5b5060a660c0565b005b60005481565b60016000808282540192505081905550565b600080819055505600a165627a7a72305820a841f5848c8c68bc957103089b41e192a79aed7ac2aebaf35ae1e36469bd44d90029',
}

resp = requests.post(url, json=json, headers=headers).json()

if "data" not in resp:
    print(f"send failed, resp: {resp}")
    exit(1)

hash = resp["data"]["hash"]

print(f"send success, hash: {hash}")

time.sleep(10)

# get onchain hash
url = target + "api/get_onchain_hash"

response = requests.post(url, json={}, headers=headers)

print(f"onchain_hash: {response.text}")
