import requests
import time

url = 'http://127.0.0.1:4000/api/send_tx'
headers1 = {
    'Content-Type': 'application/json',
    'key': 'wjjtestingautotx1'
}
json1 = {
    'user_code': "1",
    'chain_name': 'cita',
    'to': '0x1879C8B68c50A4D4eeC9852325d32B60B43f3FbD',
    'data': '0xabcd1234',
    'value': '0',
}
resp = requests.post(url, json=json1, headers=headers1).json()
if 'data' not in resp:
    print(f'resp: {resp.text}')
    exit(1)

hash = resp['data']['hash']
print(f'hash: {hash}')

time.sleep(10)

url = 'http://127.0.0.1:4000/api/get_onchain_hash'

response = requests.post(url, json={}, headers=headers1)

print(f'onchain_hash: {response.text}')