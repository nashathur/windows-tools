from dataclasses import dataclass


@dataclass(frozen=True)
class Encryption:
    encrypted: bool = False
    user_password: str | None = None
    owner_password: str | None = None
    permissions: int = 0
    restrictions_only: bool = False


def read_decision(encryption, password=None):
    if encryption.restrictions_only:
        return "open-without-password"
    return "open-with-password" if encryption.encrypted else "open-without-password"


def write_decision(encryption, *, can_modify=True, can_assemble=True, ignore_restrictions=False,
                   owner_password=None, decrypt_output=False, secret_store=None):
    warnings = []
    if decrypt_output or not encryption.encrypted:
        return {"encrypt": False, "user_password": None, "owner_password": None, "permissions": 0, "warnings": warnings}
    if encryption.restrictions_only and not owner_password and (not can_modify or not can_assemble):
        if not ignore_restrictions:
            return {"error": "ENCRYPTED", "fix": "give --owner-password-file, or --ignore-restrictions"}
        warnings.append("RESTRICTIONS_IGNORED")
    owner = owner_password
    if not owner:
        if secret_store is None:
            raise ValueError("secret_store is required when generating an owner password")
        import secrets
        owner = secrets.token_urlsafe(24)[:32]
        warnings.append("OWNER_RESET")
        secret_store.add(owner, owner=True)
    return {"encrypt": True, "method": "AES-256", "user_password": encryption.user_password,
            "owner_password": owner, "permissions": encryption.permissions, "warnings": warnings}


def combine_decision(inputs):
    encrypted = [item for item in inputs if item.encrypted]
    if not encrypted:
        return {"encryption": Encryption(), "warnings": []}
    first = encrypted[0]
    different = len(encrypted) != len(inputs) or any((x.permissions, x.user_password, x.owner_password) !=
                    (first.permissions, first.user_password, first.owner_password) for x in encrypted[1:])
    return {"encryption": first, "warnings": ["MIXED_ENCRYPTION"] if different else []}


def export_warnings(encrypted):
    return ["DECRYPTED_EXPORT"] if encrypted else []
