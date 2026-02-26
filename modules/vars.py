#🇳‌🇮‌🇰‌🇭‌🇮‌🇱‌
# Add your details here and then deploy by clicking on HEROKU Deploy button
import os
from os import environ

API_ID = int(environ.get("API_ID", "32933959"))
API_HASH = environ.get("API_HASH", "e1a5f0730a2ffc4ce45c656d9170c832")
BOT_TOKEN = environ.get("BOT_TOKEN", "7745036689:AAH0aD9Lm3iS9MjOdhLU-QijT0syjYvDgBE")

OWNER = int(environ.get("OWNER", "7562365859"))
CREDIT = environ.get("𝐂ʀᴇᴅɪᴛ", "𝐑𝐞𝐚𝐥 𝐄𝐱𝐭𝐫𝐚𝐜𝐭𝐞𝐫")
cookies_file_path = os.getenv("cookies_file_path", "youtube_cookies.txt")

TOTAL_USER = os.environ.get('TOTAL_USERS', '7562365859').split(',')
TOTAL_USERS = [int(user_id) for user_id in TOTAL_USER]

AUTH_USER = os.environ.get('AUTH_USERS', '7562365859').split(',')
AUTH_USERS = [int(user_id) for user_id in AUTH_USER]
if int(OWNER) not in AUTH_USERS:
    AUTH_USERS.append(int(OWNER))
  
# .....,.....,.......,...,.......,....., .....,.....,.......,...,.......,.....,
api_url = "http://master-api-v3.vercel.app/"
api_token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJ1c2VyX2lkIjoiNzkxOTMzNDE5NSIsInRnX3VzZXJuYW1lIjoi4p61IFtvZmZsaW5lXSIsImlhdCI6MTczODY5MjA3N30.SXzZ1MZcvMp5sGESj0hBKSghhxJ3k1GTWoBUbivUe1I"
# .....,.....,.......,...,.......,....., .....,.....,.......,...,.




