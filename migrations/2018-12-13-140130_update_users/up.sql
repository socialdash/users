UPDATE users
SET email_verified = true
WHERE id in (
	SELECT user_id
	FROM identities
	WHERE provider in ('google', 'facebook') );
