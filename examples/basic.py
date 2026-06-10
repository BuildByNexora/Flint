import flint

limiter = flint.Limiter(data_dir=".flint")
limiter.limit("api:user-42", rate=3, per="1m", algorithm="token_bucket")

for _ in range(5):
    result = limiter.check("api:user-42")
    print(result.allowed, result.remaining, result.reset_at)
