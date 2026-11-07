#!/bin/sh
# Create the small example project that docs/demo.tape records, in /tmp/lspyx-demo.
set -eu

root=/tmp/lspyx-demo
rm -rf "$root"
mkdir -p "$root/src/shop"
cd "$root"
git init -q

cat > pyproject.toml <<'EOF'
[project]
name = "shop"
version = "0.1.0"
requires-python = ">=3.12"
EOF

: > src/shop/__init__.py

cat > src/shop/models.py <<'EOF'
from dataclasses import dataclass


@dataclass
class Item:
    sku: str
    price_cents: int
    quantity: int = 1


@dataclass
class Cart:
    items: list[Item]

    def total_cents(self) -> int:
        return sum(item.price_cents * item.quantity for item in self.items)

    def add(self, item: Item) -> None:
        self.items.append(item)
EOF

cat > src/shop/checkout.py <<'EOF'
from shop.models import Cart, Item


def apply_discount(cart: Cart, percent: int) -> int:
    return cart.total_cents() * (100 - percent) // 100


def checkout(cart: Cart) -> str:
    total = apply_discount(cart, 10)
    return f"Charged {total / 100:.2f}"


def demo() -> str:
    cart = Cart(items=[Item("book", 1500)])
    cart.add(Item("pen", 250, quantity=2))
    return checkout(cart)
EOF
