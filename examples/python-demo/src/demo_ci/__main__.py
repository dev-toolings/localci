import sys

from demo_ci import greet


print(greet(sys.argv[1] if len(sys.argv) > 1 else "developer"))
