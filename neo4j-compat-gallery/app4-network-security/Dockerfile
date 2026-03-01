FROM python:3.11-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY . .

RUN useradd -r -s /bin/false appuser
USER appuser

EXPOSE 5002

CMD ["python3", "app.py"]
