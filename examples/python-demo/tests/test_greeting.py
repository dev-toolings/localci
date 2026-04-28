import unittest

from demo_ci import greet


class GreetingTest(unittest.TestCase):
    def test_named_greeting(self) -> None:
        self.assertEqual(greet("Ada"), "Hello Ada from Python localci")

    def test_default_greeting(self) -> None:
        self.assertEqual(greet(""), "Hello developer from Python localci")


if __name__ == "__main__":
    unittest.main()
