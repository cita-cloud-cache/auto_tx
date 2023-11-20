import requests
import time


def get_request_key():
    return str(int(time.time() * 1000))


def send_transaction(target, chain_name, request_key, user_code, create):
    url = f"{target}/api/{chain_name}/send_tx"
    headers = {
        "Content-Type": "application/json",
        "request_key": request_key,
        "user_code": user_code,
    }
    json_data = (
        {
            "data": "608060405234801561001057600080fd5b5060f58061001f6000396000f3006080604052600436106053576000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff16806306661abd1460585780634f2be91f146080578063d826f88f146094575b600080fd5b348015606357600080fd5b50606a60a8565b6040518082815260200191505060405180910390f35b348015608b57600080fd5b50609260ae565b005b348015609f57600080fd5b5060a660c0565b005b60005481565b60016000808282540192505081905550565b600080819055505600a165627a7a72305820a841f5848c8c68bc957103089b41e192a79aed7ac2aebaf35ae1e36469bd44d90029",
        }
        if create
        else {
            "to": "0x1879C8B68c50A4D4eeC9852325d32B60B43f3FbD",
            "data": "0xabcd1234",
        }
    )
    try:
        response = requests.post(url, json=json_data, headers=headers)
        response.raise_for_status()  # Raises HTTPError for bad responses
        data = response.json()
        if "data" not in data:
            return None, f"send failed, response: {data}"
        return data["data"]["hash"], None
    except requests.exceptions.RequestException as e:
        return None, f"HTTP error occurred: {e}"


def get_onchain_hash(target, request_key, user_code):
    url = f"{target}/api/get_onchain_hash"
    headers = {
        "request_key": request_key,
        "user_code": user_code,
    }
    max_attempts = 100
    for _ in range(max_attempts):
        try:
            time.sleep(1)
            response = requests.get(url, headers=headers)
            response.raise_for_status()
            data = response.json()
            if "data" in data:
                return data["data"], None
            if "message" in data:
                return None, data["message"]
        except requests.exceptions.RequestException as e:
            return None, f"HTTP error occurred: {e}"
    return None, "Max attempts reached, transaction not found"


target = "http://192.168.120.4/auto_tx"
chain_name = "citacloud-shuwenlian"

# target = "http://127.0.0.1:3000/"
# chain_name = "cita-test"

request_key = get_request_key()
user_code = "did:bid:zf26MwL8MVDzjSQb6UMsam4r4pVqPzP93"

hash, error = send_transaction(target, chain_name, request_key, user_code, False)
if error:
    print(error)
else:
    print(f"send success, hash: {hash}")

result, error = get_onchain_hash(target, request_key, user_code)
if error:
    print(error)
else:
    print(f"result: {result}")
